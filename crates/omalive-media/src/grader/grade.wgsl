// One pass: camera Y'CbCr -> R'G'B' -> baked grade LUT -> output encodings.
//
// The grade (and any creative LUT) is a single 3D texture sampled with
// hardware trilinear filtering, so this shader's cost does not depend on
// which controls are active.

struct Params {
    decode: array<vec4<f32>, 3>,
    encode: array<vec4<f32>, 3>,
    size: vec2<u32>,
    preview_size: vec2<u32>,
    // bit 0: NV12 out, bit 1: YUYV out, bit 2: input is YUYV (else NV12)
    flags: u32,
    lut_scale: f32,
    lut_offset: f32,
    _pad: u32,
    swatch_size: vec2<u32>,
    swatch_count: u32,
    // Points per side of each swatch lattice.
    swatch_lut: u32,
    bar_size: vec2<u32>,
    _pad2: vec2<u32>,
};

@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var luma: texture_2d<f32>;     // NV12 Y (R8), or YUYV packed (RGBA8, width/2)
@group(0) @binding(2) var chroma: texture_2d<f32>;   // NV12 CbCr (RG8, half size)
@group(0) @binding(3) var lut: texture_3d<f32>;
@group(0) @binding(4) var linear_clamp: sampler;
@group(0) @binding(5) var<storage, read_write> out_nv12: array<u32>;
@group(0) @binding(6) var<storage, read_write> out_yuyv: array<u32>;
@group(0) @binding(7) var<storage, read_write> out_preview: array<u32>;
@group(0) @binding(8) var swatch_luts: texture_3d<f32>; // swatch lattices stacked in depth
@group(0) @binding(9) var<storage, read_write> out_bar: array<u32>;

fn to_rgb(ycc: vec3<f32>) -> vec3<f32> {
    let v = vec4<f32>(ycc, 1.0);
    return clamp(vec3<f32>(dot(params.decode[0], v), dot(params.decode[1], v), dot(params.decode[2], v)),
                 vec3<f32>(0.0), vec3<f32>(1.0));
}

fn to_ycc(rgb: vec3<f32>) -> vec3<f32> {
    let v = vec4<f32>(rgb, 1.0);
    return vec3<f32>(dot(params.encode[0], v), dot(params.encode[1], v), dot(params.encode[2], v));
}

fn graded(rgb: vec3<f32>) -> vec3<f32> {
    let coordinate = rgb * params.lut_scale + vec3<f32>(params.lut_offset);
    return textureSampleLevel(lut, linear_clamp, coordinate, 0.0).rgb;
}

fn byte(v: f32) -> u32 {
    return u32(clamp(v * 255.0 + 0.5, 0.0, 255.0));
}

fn pack(a: f32, b: f32, c: f32, d: f32) -> u32 {
    return byte(a) | (byte(b) << 8u) | (byte(c) << 16u) | (byte(d) << 24u);
}

// Source sample at integer pixel (x, y).
fn source(x: u32, y: u32) -> vec3<f32> {
    if ((params.flags & 4u) != 0u) {
        let texel = textureLoad(luma, vec2<u32>(x / 2u, y), 0);
        let yv = select(texel.r, texel.b, (x & 1u) == 1u);
        return vec3<f32>(yv, texel.g, texel.a);
    }
    let yv = textureLoad(luma, vec2<u32>(x, y), 0).r;
    let c = textureLoad(chroma, vec2<u32>(x / 2u, y / 2u), 0).rg;
    return vec3<f32>(yv, c.r, c.g);
}

// Each invocation grades a 4x2 block so every output word has one writer.
@compute @workgroup_size(8, 8)
fn grade(@builtin(global_invocation_id) id: vec3<u32>) {
    let width = params.size.x;
    let height = params.size.y;
    if (id.x * 4u >= width || id.y * 2u >= height) {
        return;
    }
    let x0 = id.x * 4u;
    let y0 = id.y * 2u;
    var ycc: array<vec3<f32>, 8>;
    for (var row = 0u; row < 2u; row++) {
        for (var col = 0u; col < 4u; col++) {
            ycc[row * 4u + col] = to_ycc(graded(to_rgb(source(x0 + col, y0 + row))));
        }
    }
    if ((params.flags & 1u) != 0u) {
        for (var row = 0u; row < 2u; row++) {
            let r = row * 4u;
            out_nv12[((y0 + row) * width + x0) / 4u] =
                pack(ycc[r].x, ycc[r + 1u].x, ycc[r + 2u].x, ycc[r + 3u].x);
        }
        // Chroma averaged over each 2x2 neighbourhood.
        let left = (ycc[0].yz + ycc[1].yz + ycc[4].yz + ycc[5].yz) * 0.25;
        let right = (ycc[2].yz + ycc[3].yz + ycc[6].yz + ycc[7].yz) * 0.25;
        out_nv12[(width * height + id.y * width + x0) / 4u] = pack(left.x, left.y, right.x, right.y);
    }
    if ((params.flags & 2u) != 0u) {
        for (var row = 0u; row < 2u; row++) {
            let r = row * 4u;
            let base = ((y0 + row) * width + x0) / 2u;
            let a = (ycc[r].yz + ycc[r + 1u].yz) * 0.5;
            let b = (ycc[r + 2u].yz + ycc[r + 3u].yz) * 0.5;
            out_yuyv[base] = pack(ycc[r].x, a.x, ycc[r + 1u].x, a.y);
            out_yuyv[base + 1u] = pack(ycc[r + 2u].x, b.x, ycc[r + 3u].x, b.y);
        }
    }
}

// The ungraded R'G'B' at normalised position `uv`, filtered for NV12.
fn source_at(uv: vec2<f32>) -> vec3<f32> {
    if ((params.flags & 4u) != 0u) {
        let pixel = vec2<u32>(uv * vec2<f32>(params.size));
        return to_rgb(source(min(pixel.x, params.size.x - 1u), min(pixel.y, params.size.y - 1u)));
    }
    let yv = textureSampleLevel(luma, linear_clamp, uv, 0.0).r;
    let c = textureSampleLevel(chroma, linear_clamp, uv, 0.0).rg;
    return to_rgb(vec3<f32>(yv, c.r, c.g));
}

// Downscaled BGRA previews for the UI, filtered from the source planes:
// graded first, then the original (for the before/after split).
@compute @workgroup_size(8, 8)
fn preview(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = params.preview_size;
    if (id.x >= size.x || id.y >= size.y) {
        return;
    }
    let uv = (vec2<f32>(id.xy) + vec2<f32>(0.5)) / vec2<f32>(size);
    let original = source_at(uv);
    let rgb = graded(original);
    let index = id.y * size.x + id.x;
    out_preview[index] = pack(rgb.b, rgb.g, rgb.r, 1.0);
    out_preview[size.x * size.y + index] = pack(original.b, original.g, original.r, 1.0);
}

// One small BGRA image per look (z picks the look), after the preview and
// the original in the same buffer. Each look's lattice is its own slab of
// the stacked texture; coordinates stay between its first and last texel
// centres so filtering never blends neighbouring looks.
@compute @workgroup_size(8, 8)
fn swatches(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = params.swatch_size;
    if (id.x >= size.x || id.y >= size.y || id.z >= params.swatch_count) {
        return;
    }
    let uv = (vec2<f32>(id.xy) + vec2<f32>(0.5)) / vec2<f32>(size);
    let n = f32(params.swatch_lut);
    let c = source_at(uv) * ((n - 1.0) / n) + vec3<f32>(0.5 / n);
    let slabs = f32(textureDimensions(swatch_luts).z) / n;
    let coordinate = vec3<f32>(c.xy, (f32(id.z) + c.z) / slabs);
    let rgb = textureSampleLevel(swatch_luts, linear_clamp, coordinate, 0.0).rgb;
    let base = 2u * params.preview_size.x * params.preview_size.y;
    out_preview[base + (id.z * size.y + id.y) * size.x + id.x] = pack(rgb.b, rgb.g, rgb.r, 1.0);
}

// The bar's RGBA picture. Four filtered taps spread across each pixel's
// footprint average about a 4×4 block of a 1080p source, so fine detail
// shrinks without shimmer.
@compute @workgroup_size(8, 8)
fn bar(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = params.bar_size;
    if (id.x >= size.x || id.y >= size.y) {
        return;
    }
    let step = 1.0 / vec2<f32>(size);
    let centre = (vec2<f32>(id.xy) + vec2<f32>(0.5)) * step;
    var sum = vec3<f32>(0.0);
    for (var i = 0u; i < 4u; i++) {
        let offset = vec2<f32>(f32(i & 1u), f32(i >> 1u)) * 0.5 - vec2<f32>(0.25);
        sum += source_at(centre + offset * step);
    }
    let rgb = graded(sum * 0.25);
    out_bar[id.y * size.x + id.x] = pack(rgb.r, rgb.g, rgb.b, 1.0);
}
