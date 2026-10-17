//! GPU grader: one compute dispatch per frame, outputs read back through
//! mapped buffers (cheap on unified-memory GPUs such as Apple silicon).
use super::{BAR, Input, InputFormat, Outputs, SWATCH, SWATCH_LUT, Wants};
use anyhow::{Context, Result, bail};
use omalive_core::{Lattice, color::YuvMatrix};

pub struct GpuGrader {
    device: wgpu::Device,
    queue: wgpu::Queue,
    grade: wgpu::ComputePipeline,
    preview: wgpu::ComputePipeline,
    swatches: wgpu::ComputePipeline,
    bar: wgpu::ComputePipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    lut: wgpu::Texture,
    lut_size: u32,
    /// The swatch looks, stacked along the third axis.
    swatch_luts: wgpu::Texture,
    swatch_count: u32,
    params: wgpu::Buffer,
    frame: Option<FrameResources>,
    pub adapter: String,
}

/// Source size and format, preview size, swatch count.
type FrameKey = (u32, u32, InputFormat, u32, u32, u32);

struct FrameResources {
    key: FrameKey,
    luma: wgpu::Texture,
    chroma: wgpu::Texture,
    nv12: wgpu::Buffer,
    yuyv: wgpu::Buffer,
    preview: wgpu::Buffer,
    bar: wgpu::Buffer,
    staging: [wgpu::Buffer; 4],
    bind: wgpu::BindGroup,
}

impl GpuGrader {
    /// A hardware adapter only; software rasterisers are slower than the
    /// CPU path and are rejected.
    pub fn new() -> Result<Self> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN | wgpu::Backends::GL,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            force_fallback_adapter: false,
            compatible_surface: None,
        }))
        .context("No GPU adapter")?;
        let info = adapter.get_info();
        if info.device_type == wgpu::DeviceType::Cpu {
            bail!("Only a software renderer ({}) is available", info.name);
        }
        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
                label: Some("omalive-grader"),
                required_limits:
                    wgpu::Limits::downlevel_defaults().using_resolution(adapter.limits()),
                ..Default::default()
            }))?;
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("grade.wgsl"),
            source: wgpu::ShaderSource::Wgsl(include_str!("grade.wgsl").into()),
        });
        let texture = |binding, dimension, filterable| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable },
                view_dimension: dimension,
                multisampled: false,
            },
            count: None,
        };
        let storage = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only: false },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("grade"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                texture(1, wgpu::TextureViewDimension::D2, true),
                texture(2, wgpu::TextureViewDimension::D2, true),
                texture(3, wgpu::TextureViewDimension::D3, true),
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                storage(5),
                storage(6),
                storage(7),
                texture(8, wgpu::TextureViewDimension::D3, true),
                storage(9),
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("grade"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = |entry| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(entry),
                layout: Some(&pipeline_layout),
                module: &module,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                cache: None,
            })
        };
        let grade = pipeline("grade");
        let preview = pipeline("preview");
        let swatches = pipeline("swatches");
        let bar = pipeline("bar");
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("linear"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let params = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("params"),
            size: 160,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let identity = Lattice::identity(2);
        let lut = lut_texture(&device, &queue, &identity);
        let swatch_luts = lut_texture(&device, &queue, &identity);
        Ok(Self {
            device,
            queue,
            grade,
            preview,
            swatches,
            bar,
            layout,
            sampler,
            lut,
            lut_size: 2,
            swatch_luts,
            swatch_count: 0,
            params,
            frame: None,
            adapter: info.name,
        })
    }

    pub fn set_lattice(&mut self, lattice: &Lattice) {
        if lattice.size as u32 != self.lut_size {
            self.lut = lut_texture(&self.device, &self.queue, lattice);
            self.lut_size = lattice.size as u32;
            self.frame = None;
        } else {
            write_lut(&self.queue, &self.lut, lattice);
        }
    }

    /// Lattices of [`SWATCH_LUT`] points each.
    pub fn set_swatches(&mut self, lattices: &[Lattice]) {
        let count = lattices.len() as u32;
        if count == 0 {
            self.swatch_count = 0;
            return;
        }
        let size = SWATCH_LUT as u32;
        let extent = wgpu::Extent3d {
            width: size,
            height: size,
            depth_or_array_layers: size * count,
        };
        if count != self.swatch_count || self.swatch_luts.size() != extent {
            self.swatch_luts = self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("swatch-luts"),
                size: extent,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D3,
                format: wgpu::TextureFormat::Rgba16Float,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            self.swatch_count = count;
            self.frame = None;
        }
        for (i, lattice) in lattices.iter().enumerate() {
            debug_assert_eq!(lattice.size, SWATCH_LUT);
            write_lattice(
                &self.queue,
                &self.swatch_luts,
                lattice,
                wgpu::Origin3d {
                    x: 0,
                    y: 0,
                    z: i as u32 * size,
                },
            );
        }
    }

    pub fn process(
        &mut self,
        input: &Input,
        matrix: YuvMatrix,
        wants: Wants,
        out: &mut Outputs,
    ) -> Result<()> {
        let (width, height) = (input.width, input.height);
        if width % 4 != 0 || height % 2 != 0 {
            bail!("Camera size {width}×{height} must be a multiple of 4×2");
        }
        let (pw, ph) = wants.preview.unwrap_or((0, 0));
        let swatches = if pw > 0 {
            wants.swatches.min(self.swatch_count)
        } else {
            0
        };
        let key = (width, height, input.format, pw, ph, self.swatch_count);
        if self.frame.as_ref().is_none_or(|f| f.key != key) {
            self.frame = Some(self.resources(key));
        }
        let frame = self.frame.as_ref().expect("frame resources");
        // Upload source planes.
        match input.format {
            InputFormat::Nv12 => {
                let stride = input.stride;
                upload(&self.queue, &frame.luma, input.data, 0, stride, height);
                upload(
                    &self.queue,
                    &frame.chroma,
                    input.data,
                    stride as usize * height as usize,
                    stride,
                    height / 2,
                );
            }
            InputFormat::Yuyv => {
                upload(
                    &self.queue,
                    &frame.luma,
                    input.data,
                    0,
                    input.stride,
                    height,
                );
            }
        }
        let flags = u32::from(wants.nv12)
            | (u32::from(wants.yuyv) << 1)
            | (u32::from(input.format == InputFormat::Yuyv) << 2);
        let scale = (self.lut_size - 1) as f32 / self.lut_size as f32;
        let offset = 0.5 / self.lut_size as f32;
        self.queue.write_buffer(
            &self.params,
            0,
            &params_bytes(
                matrix,
                (width, height),
                (pw, ph),
                flags,
                scale,
                offset,
                swatches,
            ),
        );
        let mut encoder = self.device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_compute_pass(&Default::default());
            pass.set_bind_group(0, &frame.bind, &[]);
            if wants.nv12 || wants.yuyv {
                pass.set_pipeline(&self.grade);
                pass.dispatch_workgroups((width / 4).div_ceil(8), (height / 2).div_ceil(8), 1);
            }
            if pw > 0 {
                pass.set_pipeline(&self.preview);
                pass.dispatch_workgroups(pw.div_ceil(8), ph.div_ceil(8), 1);
            }
            if swatches > 0 {
                pass.set_pipeline(&self.swatches);
                pass.dispatch_workgroups(SWATCH.0.div_ceil(8), SWATCH.1.div_ceil(8), swatches);
            }
            if wants.bar {
                pass.set_pipeline(&self.bar);
                pass.dispatch_workgroups(BAR.0.div_ceil(8), BAR.1.div_ceil(8), 1);
            }
        }
        let copies = [
            (wants.nv12, &frame.nv12, &frame.staging[0]),
            (wants.yuyv, &frame.yuyv, &frame.staging[1]),
            (pw > 0, &frame.preview, &frame.staging[2]),
            (wants.bar, &frame.bar, &frame.staging[3]),
        ];
        for (wanted, source, staging) in copies {
            if wanted {
                encoder.copy_buffer_to_buffer(source, 0, staging, 0, source.size());
            }
        }
        let submission = self.queue.submit([encoder.finish()]);
        for (wanted, _, staging) in copies {
            if wanted {
                staging.slice(..).map_async(wgpu::MapMode::Read, |_| {});
            }
        }
        self.device.poll(wgpu::PollType::Wait {
            submission_index: Some(submission),
            timeout: Some(std::time::Duration::from_secs(1)),
        })?;
        out.bar.clear();
        let targets: [(bool, &mut Vec<u8>, usize); 4] = [
            (
                wants.nv12,
                &mut out.nv12,
                width as usize * height as usize * 3 / 2,
            ),
            (
                wants.yuyv,
                &mut out.yuyv,
                width as usize * height as usize * 2,
            ),
            (
                pw > 0,
                &mut out.preview,
                pw as usize * ph as usize * 8 + swatches as usize * swatch_bytes(),
            ),
            (wants.bar, &mut out.bar, BAR.0 as usize * BAR.1 as usize * 4),
        ];
        for ((wanted, target, length), staging) in targets.into_iter().zip(&frame.staging) {
            if wanted {
                {
                    let view = staging.slice(..length as u64).get_mapped_range();
                    target.clear();
                    target.extend_from_slice(&view);
                }
                staging.unmap();
            }
        }
        out.swatches.clear();
        if pw > 0 {
            // The preview buffer holds the graded image, the original, then
            // the swatches.
            let half = pw as usize * ph as usize * 4;
            out.swatches.extend_from_slice(&out.preview[half * 2..]);
            out.original.clear();
            out.original.extend_from_slice(&out.preview[half..half * 2]);
            out.preview.truncate(half);
        }
        out.preview_size = (pw, ph);
        Ok(())
    }

    fn resources(&self, key: FrameKey) -> FrameResources {
        let (width, height, format, pw, ph, swatches) = key;
        let device = &self.device;
        let texture = |label, w: u32, h: u32, format| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width: w.max(1),
                    height: h.max(1),
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            })
        };
        let (luma, chroma) = match format {
            InputFormat::Nv12 => (
                texture("luma", width, height, wgpu::TextureFormat::R8Unorm),
                texture(
                    "chroma",
                    width / 2,
                    height / 2,
                    wgpu::TextureFormat::Rg8Unorm,
                ),
            ),
            InputFormat::Yuyv => (
                texture("packed", width / 2, height, wgpu::TextureFormat::Rgba8Unorm),
                texture("unused", 1, 1, wgpu::TextureFormat::Rg8Unorm),
            ),
        };
        let pixels = width as u64 * height as u64;
        let buffer = |label, size: u64, usage| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: size.max(4).next_multiple_of(4),
                usage,
                mapped_at_creation: false,
            })
        };
        let storage = wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC;
        let read = wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST;
        let nv12 = buffer("nv12", pixels * 3 / 2, storage);
        let yuyv = buffer("yuyv", pixels * 2, storage);
        let preview = buffer(
            "preview",
            pw as u64 * ph as u64 * 8 + swatches as u64 * swatch_bytes() as u64,
            storage,
        );
        let bar = buffer("bar", BAR.0 as u64 * BAR.1 as u64 * 4, storage);
        let staging = [
            buffer("nv12-read", nv12.size(), read),
            buffer("yuyv-read", yuyv.size(), read),
            buffer("preview-read", preview.size(), read),
            buffer("bar-read", bar.size(), read),
        ];
        let lut_view = self.lut.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D3),
            ..Default::default()
        });
        let swatch_view = self.swatch_luts.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D3),
            ..Default::default()
        });
        let luma_view = luma.create_view(&Default::default());
        let chroma_view = chroma.create_view(&Default::default());
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("grade"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.params.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&luma_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&chroma_view),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(&lut_view),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: nv12.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: yuyv.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 7,
                    resource: preview.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 8,
                    resource: wgpu::BindingResource::TextureView(&swatch_view),
                },
                wgpu::BindGroupEntry {
                    binding: 9,
                    resource: bar.as_entire_binding(),
                },
            ],
        });
        FrameResources {
            key,
            luma,
            chroma,
            nv12,
            yuyv,
            preview,
            bar,
            staging,
            bind,
        }
    }
}

fn upload(
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    data: &[u8],
    offset: usize,
    stride: u32,
    height: u32,
) {
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &data[offset..],
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(stride),
            rows_per_image: Some(height),
        },
        texture.size(),
    );
}

fn lut_texture(device: &wgpu::Device, queue: &wgpu::Queue, lattice: &Lattice) -> wgpu::Texture {
    let size = lattice.size as u32;
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("lut"),
        size: wgpu::Extent3d {
            width: size,
            height: size,
            depth_or_array_layers: size,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D3,
        format: wgpu::TextureFormat::Rgba16Float,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    write_lut(queue, &texture, lattice);
    texture
}

fn write_lut(queue: &wgpu::Queue, texture: &wgpu::Texture, lattice: &Lattice) {
    write_lattice(queue, texture, lattice, wgpu::Origin3d::ZERO);
}

/// One lattice into `texture` at `origin`.
fn write_lattice(
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    lattice: &Lattice,
    origin: wgpu::Origin3d,
) {
    let size = lattice.size as u32;
    let bytes: Vec<u8> = lattice
        .data
        .iter()
        .flat_map(|rgb| [rgb[0], rgb[1], rgb[2], 1.])
        .flat_map(|v| half::f16::from_f32(v).to_le_bytes())
        .collect();
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin,
            aspect: wgpu::TextureAspect::All,
        },
        &bytes,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(size * 8),
            rows_per_image: Some(size),
        },
        wgpu::Extent3d {
            width: size,
            height: size,
            depth_or_array_layers: size,
        },
    );
}

fn params_bytes(
    matrix: YuvMatrix,
    size: (u32, u32),
    preview: (u32, u32),
    flags: u32,
    scale: f32,
    offset: f32,
    swatches: u32,
) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(160);
    for row in matrix
        .decode_matrix()
        .iter()
        .chain(&YuvMatrix::BT601.encode_matrix())
    {
        for value in row {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
    }
    for value in [size.0, size.1, preview.0, preview.1, flags] {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes.extend_from_slice(&scale.to_le_bytes());
    bytes.extend_from_slice(&offset.to_le_bytes());
    bytes.extend_from_slice(&0u32.to_le_bytes());
    for value in [
        SWATCH.0,
        SWATCH.1,
        swatches,
        SWATCH_LUT as u32,
        BAR.0,
        BAR.1,
        0,
        0,
    ] {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes
}

/// Bytes in one BGRA swatch.
fn swatch_bytes() -> usize {
    SWATCH.0 as usize * SWATCH.1 as usize * 4
}
