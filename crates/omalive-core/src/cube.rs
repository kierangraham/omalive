//! Adobe/Resolve `.cube` 3D LUT reader and the per-user LUT library.
use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};

const MAX_SIZE: usize = 129;
const MAX_FILE: u64 = 64 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq)]
pub struct Cube {
    pub title: String,
    pub size: usize,
    pub domain_min: [f32; 3],
    pub domain_max: [f32; 3],
    data: Vec<[f32; 3]>,
}

impl Cube {
    pub fn read(path: &Path) -> Result<Self> {
        let length = std::fs::metadata(path)?.len();
        if length > MAX_FILE {
            bail!("{} is too large for a LUT", path.display());
        }
        let text =
            std::fs::read_to_string(path).with_context(|| format!("Reading {}", path.display()))?;
        Self::parse(&text)
            .with_context(|| format!("{} is not a valid 3D .cube LUT", file_name(path)))
    }

    pub fn parse(text: &str) -> Result<Self> {
        let mut title = String::new();
        let mut size = None;
        let mut domain_min = [0.; 3];
        let mut domain_max = [1.; 3];
        let mut data = Vec::new();
        for (number, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let mut words = line.split_whitespace();
            let first = words.next().unwrap_or_default();
            let triple = |words: std::str::SplitWhitespace| -> Result<[f32; 3]> {
                let values: Vec<f32> = words
                    .map(|w| w.parse::<f32>())
                    .collect::<Result<_, _>>()
                    .with_context(|| format!("line {}", number + 1))?;
                match values[..] {
                    [r, g, b] if values.iter().all(|v| v.is_finite()) => Ok([r, g, b]),
                    _ => bail!("line {} needs three numbers", number + 1),
                }
            };
            match first {
                "TITLE" => title = line[5..].trim().trim_matches('"').to_string(),
                "LUT_3D_SIZE" => {
                    let value: usize = words.next().unwrap_or_default().parse()?;
                    if !(2..=MAX_SIZE).contains(&value) {
                        bail!("LUT_3D_SIZE {value} is outside 2…{MAX_SIZE}");
                    }
                    size = Some(value);
                    data.reserve(value * value * value);
                }
                "LUT_1D_SIZE" => bail!("1D LUTs are not supported; export a 3D LUT"),
                "DOMAIN_MIN" => domain_min = triple(words)?,
                "DOMAIN_MAX" => domain_max = triple(words)?,
                "LUT_3D_INPUT_RANGE" | "LUT_1D_INPUT_RANGE" => {
                    let values: Vec<f32> = words.map(|w| w.parse()).collect::<Result<_, _>>()?;
                    if let [low, high] = values[..] {
                        domain_min = [low; 3];
                        domain_max = [high; 3];
                    }
                }
                word if word.starts_with(|c: char| c.is_ascii_digit() || c == '-' || c == '.') => {
                    if size.is_none() {
                        bail!("data before LUT_3D_SIZE");
                    }
                    data.push(triple(line.split_whitespace())?);
                }
                // Unknown keywords are vendor extensions; ignore them.
                _ => {}
            }
        }
        let size = size.context("missing LUT_3D_SIZE")?;
        if data.len() != size * size * size {
            bail!(
                "expected {} entries, found {}",
                size * size * size,
                data.len()
            );
        }
        if (0..3).any(|i| domain_max[i] <= domain_min[i]) {
            bail!("DOMAIN_MAX must exceed DOMAIN_MIN");
        }
        Ok(Self {
            title,
            size,
            domain_min,
            domain_max,
            data,
        })
    }

    pub fn from_fn(size: usize, f: impl Fn([f32; 3]) -> [f32; 3]) -> Self {
        let scale = 1. / (size - 1) as f32;
        let mut data = Vec::with_capacity(size * size * size);
        for b in 0..size {
            for g in 0..size {
                for r in 0..size {
                    data.push(f([r as f32 * scale, g as f32 * scale, b as f32 * scale]));
                }
            }
        }
        Self {
            title: String::new(),
            size,
            domain_min: [0.; 3],
            domain_max: [1.; 3],
            data,
        }
    }

    /// A cube from a Hald CLUT image (8-bit RGB, row by row). A level-L
    /// Hald image is L³ × L³ pixels holding an L²-sized cube with red
    /// varying fastest, then green, then blue: the `.cube` order, so pixel
    /// `i` is lattice entry `i`.
    pub fn from_hald(width: u32, height: u32, rgb: &[u8]) -> Result<Self> {
        let level = (2..=16u32).find(|l| l * l * l == width).with_context(|| {
            format!("A {width}×{height} image is not a Hald CLUT (expected level² × level³ pixels)")
        })?;
        if width != height || rgb.len() != (width * height * 3) as usize {
            bail!("A Hald CLUT must be square RGB; this is {width}×{height}");
        }
        let size = (level * level) as usize;
        let data = rgb
            .chunks_exact(3)
            .take(size * size * size)
            .map(|p| [p[0], p[1], p[2]].map(|v| v as f32 / 255.))
            .collect();
        Ok(Self {
            title: String::new(),
            size,
            domain_min: [0.; 3],
            domain_max: [1.; 3],
            data,
        })
    }

    /// The same look on a `size`-point lattice (trilinear). Film-emulation
    /// Hald CLUTs are often 144³; 33³ is visually identical for smooth
    /// looks and a fraction of the size.
    pub fn resample(&self, size: usize) -> Self {
        let mut cube = Self::from_fn(size, |rgb| self.sample(rgb));
        cube.title = self.title.clone();
        cube
    }

    /// The cube in `.cube` text form.
    pub fn to_text(&self) -> String {
        use std::fmt::Write;
        let mut text = String::new();
        if !self.title.is_empty() {
            let _ = writeln!(text, "TITLE \"{}\"", self.title);
        }
        let _ = writeln!(text, "LUT_3D_SIZE {}", self.size);
        let [a, b, c] = self.domain_min;
        let _ = writeln!(text, "DOMAIN_MIN {a} {b} {c}");
        let [a, b, c] = self.domain_max;
        let _ = writeln!(text, "DOMAIN_MAX {a} {b} {c}");
        for [r, g, b] in &self.data {
            let _ = writeln!(text, "{r:.6} {g:.6} {b:.6}");
        }
        text
    }

    pub fn sample(&self, rgb: [f32; 3]) -> [f32; 3] {
        let normalized: [f32; 3] = std::array::from_fn(|i| {
            (rgb[i] - self.domain_min[i]) / (self.domain_max[i] - self.domain_min[i])
        });
        crate::grade::trilinear(&self.data, self.size, normalized)
    }
}

/// `~/.config/omalive/luts`
pub fn library_dir() -> PathBuf {
    crate::settings::config_dir().join("luts")
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LutEntry {
    pub file: String,
    pub name: String,
}

/// `.cube` files in the library, sorted by display name.
pub fn library(dir: &Path) -> Vec<LutEntry> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut luts: Vec<LutEntry> = entries
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            entry
                .file_type()
                .is_ok_and(|t| t.is_file() || t.is_symlink())
        })
        .filter_map(|entry| {
            let file = entry.file_name().into_string().ok()?;
            let stem = file
                .strip_suffix(".cube")
                .or_else(|| file.strip_suffix(".CUBE"))?;
            let name = stem.replace(['_', '-'], " ");
            (!file.starts_with('.')).then_some(LutEntry { name, file })
        })
        .collect();
    luts.sort_by_key(|lut| lut.name.to_lowercase());
    luts
}

/// Validate and copy a LUT into the library, returning its file name there.
pub fn import(source: &Path, dir: &Path) -> Result<String> {
    Cube::read(source)?;
    let name = source
        .file_name()
        .and_then(|n| n.to_str())
        .filter(|n| !n.starts_with('.'))
        .context("LUT file name must be valid UTF-8")?;
    let name = if name.to_lowercase().ends_with(".cube") {
        name.to_string()
    } else {
        format!("{name}.cube")
    };
    std::fs::create_dir_all(dir)?;
    let target = dir.join(&name);
    if target != source {
        std::fs::copy(source, &target)
            .with_context(|| format!("Copying {} into the LUT library", file_name(source)))?;
    }
    Ok(name)
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_identity_hald_clut_becomes_an_identity_cube() {
        // Level 2: an 8×8 image holding a 4³ cube.
        let size = 4;
        let mut rgb = Vec::new();
        for b in 0..size {
            for g in 0..size {
                for r in 0..size {
                    rgb.extend([r, g, b].map(|v| (v * 255 / (size - 1)) as u8));
                }
            }
        }
        let cube = Cube::from_hald(8, 8, &rgb).unwrap();
        assert_eq!(cube.size, 4);
        for rgb in [[0.2, 0.5, 0.9], [1., 0., 0.33]] {
            let out = cube.sample(rgb);
            for (a, b) in out.iter().zip(rgb) {
                assert!((a - b).abs() < 0.01, "{out:?}");
            }
        }
        let small = cube.resample(3);
        assert_eq!(small.size, 3);
        assert!((small.sample([0.5, 0.25, 0.75])[1] - 0.25).abs() < 0.01);
        assert!(Cube::from_hald(10, 10, &[0; 300]).is_err());
        assert!(Cube::from_hald(8, 4, &[0; 96]).is_err());
    }

    fn identity_text(size: usize) -> String {
        let mut text = format!("# comment\nTITLE \"Identity\"\nLUT_3D_SIZE {size}\n");
        let scale = 1. / (size - 1) as f32;
        for b in 0..size {
            for g in 0..size {
                for r in 0..size {
                    text += &format!(
                        "{} {} {}\n",
                        r as f32 * scale,
                        g as f32 * scale,
                        b as f32 * scale
                    );
                }
            }
        }
        text
    }

    #[test]
    fn parses_and_samples_identity() {
        let cube = Cube::parse(&identity_text(5)).unwrap();
        assert_eq!(cube.title, "Identity");
        assert_eq!(cube.size, 5);
        let out = cube.sample([0.13, 0.5, 0.92]);
        assert!(
            out.iter()
                .zip([0.13, 0.5, 0.92])
                .all(|(a, b)| (a - b).abs() < 1e-5)
        );
    }

    #[test]
    fn rejects_malformed_luts() {
        assert!(Cube::parse("LUT_3D_SIZE 2\n0 0 0\n").is_err());
        assert!(Cube::parse("LUT_1D_SIZE 1024\n").is_err());
        assert!(Cube::parse("0 0 0\n").is_err());
        assert!(Cube::parse("LUT_3D_SIZE 4000\n").is_err());
        let mut text = identity_text(2);
        text = text.replacen("0 0 0", "nan 0 0", 1);
        assert!(Cube::parse(&text).is_err());
    }

    #[test]
    fn domain_is_respected() {
        let mut text = identity_text(2).replace(
            "LUT_3D_SIZE 2",
            "LUT_3D_SIZE 2\nDOMAIN_MIN 0 0 0\nDOMAIN_MAX 2 2 2",
        );
        text.push('\n');
        let cube = Cube::parse(&text).unwrap();
        assert!((cube.sample([1., 1., 1.])[0] - 0.5).abs() < 1e-5);
    }

    #[test]
    fn library_lists_and_imports() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("Warm_Film.cube");
        std::fs::write(&source, identity_text(2)).unwrap();
        let library_dir = dir.path().join("luts");
        assert_eq!(import(&source, &library_dir).unwrap(), "Warm_Film.cube");
        std::fs::write(library_dir.join("notes.txt"), "x").unwrap();
        let luts = library(&library_dir);
        assert_eq!(
            luts,
            [LutEntry {
                file: "Warm_Film.cube".into(),
                name: "Warm Film".into()
            }]
        );
        let bad = dir.path().join("bad.cube");
        std::fs::write(&bad, "nope").unwrap();
        assert!(import(&bad, &library_dir).is_err());
    }

    #[test]
    fn text_round_trips() {
        let cube = Cube::from_fn(5, |[r, g, b]| [g, b * 0.5, r]);
        let parsed = Cube::parse(&cube.to_text()).unwrap();
        assert_eq!(parsed.size, 5);
        let out = parsed.sample([0.25, 0.5, 1.]);
        assert!(
            (out[0] - 0.5).abs() < 1e-4
                && (out[1] - 0.5).abs() < 1e-4
                && (out[2] - 0.25).abs() < 1e-4
        );
    }
}
