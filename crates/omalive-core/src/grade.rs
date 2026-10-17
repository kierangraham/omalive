//! Camera colour correction, baked into a single 3D LUT.
//!
//! Every control here is a pure per-pixel colour function, so the whole grade
//! (plus an optional creative LUT at a chosen strength) collapses into one
//! lattice that the GPU samples once per pixel. Changing a slider costs one
//! bake (well under a millisecond); frames cost the same regardless of how
//! many controls are active.
use crate::cube::Cube;
use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

/// Lattice size of the baked LUT. 33 is the industry default: hardware
/// trilinear filtering between nodes is visually exact for smooth grades.
pub const BAKE_SIZE: usize = 33;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Grade {
    /// Stops, −2…+2.
    pub exposure: f32,
    /// −1…+1 for the remaining controls; 0 is neutral.
    pub contrast: f32,
    pub highlights: f32,
    pub shadows: f32,
    pub warmth: f32,
    pub tint: f32,
    pub vibrance: f32,
    pub saturation: f32,
}

impl Default for Grade {
    fn default() -> Self {
        Self {
            exposure: 0.,
            contrast: 0.,
            highlights: 0.,
            shadows: 0.,
            warmth: 0.,
            tint: 0.,
            vibrance: 0.,
            saturation: 0.,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Control {
    Exposure,
    Contrast,
    Highlights,
    Shadows,
    Warmth,
    Tint,
    Vibrance,
    Saturation,
}

impl Control {
    pub const ALL: [Control; 8] = [
        Control::Exposure,
        Control::Contrast,
        Control::Highlights,
        Control::Shadows,
        Control::Warmth,
        Control::Tint,
        Control::Vibrance,
        Control::Saturation,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Control::Exposure => "Exposure",
            Control::Contrast => "Contrast",
            Control::Highlights => "Highlights",
            Control::Shadows => "Shadows",
            Control::Warmth => "Warmth",
            Control::Tint => "Tint",
            Control::Vibrance => "Vibrance",
            Control::Saturation => "Saturation",
        }
    }
    pub fn key(self) -> &'static str {
        match self {
            Control::Exposure => "exposure",
            Control::Contrast => "contrast",
            Control::Highlights => "highlights",
            Control::Shadows => "shadows",
            Control::Warmth => "warmth",
            Control::Tint => "tint",
            Control::Vibrance => "vibrance",
            Control::Saturation => "saturation",
        }
    }
    /// (min, max, step) in stored units.
    pub fn range(self) -> (f32, f32, f32) {
        match self {
            Control::Exposure => (-2., 2., 0.05),
            _ => (-1., 1., 0.01),
        }
    }
    /// Human-readable value: stops for exposure, signed percent otherwise.
    pub fn format(self, value: f32) -> String {
        // Snapping can yield −0.0; never show a signed zero.
        let value = if value == 0. { 0. } else { value };
        match self {
            Control::Exposure => format!("{value:+.2} EV"),
            _ => {
                let percent = (value * 100.).round() as i32;
                if percent == 0 {
                    "0".into()
                } else {
                    format!("{percent:+}")
                }
            }
        }
    }
}

impl Grade {
    pub fn get(&self, control: Control) -> f32 {
        match control {
            Control::Exposure => self.exposure,
            Control::Contrast => self.contrast,
            Control::Highlights => self.highlights,
            Control::Shadows => self.shadows,
            Control::Warmth => self.warmth,
            Control::Tint => self.tint,
            Control::Vibrance => self.vibrance,
            Control::Saturation => self.saturation,
        }
    }

    pub fn set(&mut self, control: Control, value: f32) {
        let (min, max, step) = control.range();
        let value = if value.is_finite() {
            (((value / step).round() * step) * 1000.).round() / 1000.
        } else {
            0.
        }
        .clamp(min, max);
        *match control {
            Control::Exposure => &mut self.exposure,
            Control::Contrast => &mut self.contrast,
            Control::Highlights => &mut self.highlights,
            Control::Shadows => &mut self.shadows,
            Control::Warmth => &mut self.warmth,
            Control::Tint => &mut self.tint,
            Control::Vibrance => &mut self.vibrance,
            Control::Saturation => &mut self.saturation,
        } = value;
    }

    pub fn is_neutral(&self) -> bool {
        *self == Grade::default()
    }

    pub fn validate(&self) -> Result<()> {
        for control in Control::ALL {
            let (min, max, _) = control.range();
            let value = self.get(control);
            if !value.is_finite() || value < min || value > max {
                bail!("{} is out of range", control.label());
            }
        }
        Ok(())
    }

    /// Grade one display-referred RGB value in 0…1.
    pub fn apply(&self, rgb: [f32; 3]) -> [f32; 3] {
        let curves = self.curves();
        self.mix([0, 1, 2].map(|c| curves.channel(c, rgb[c])))
    }

    /// The per-channel part of the grade: white balance, exposure and tone.
    fn curves(&self) -> Curves {
        let gains = [
            1. + 0.22 * self.warmth,
            1. - 0.18 * self.tint,
            1. - 0.22 * self.warmth,
        ];
        let norm = luma(gains);
        let exposure = 2f32.powf(self.exposure);
        // When gain can push white past 1, fade in a soft shoulder instead of
        // hard clipping. At or below unity gain the transfer stays exact.
        let peak = gains.iter().fold(0f32, |m, g| m.max(*g)) / norm * exposure;
        Curves {
            gains: gains.map(|g| g / norm * exposure),
            rolloff: ((peak - 1.) * 4.).clamp(0., 1.),
            contrast: self.contrast,
            highlights: self.highlights,
            shadows: self.shadows,
        }
    }

    /// The cross-channel part: vibrance and saturation around luma.
    fn mix(&self, rgb: [f32; 3]) -> [f32; 3] {
        if self.vibrance == 0. && self.saturation == 0. {
            return rgb;
        }
        let y = luma(rgb);
        let chroma =
            rgb.iter().fold(0f32, |m, v| m.max(*v)) - rgb.iter().fold(1f32, |m, v| m.min(*v));
        // Vibrance favours muted colours (skin stays natural).
        let vibrance = 1. + self.vibrance * (1. - chroma.clamp(0., 1.)).powi(2);
        let factor = (1. + self.saturation) * vibrance;
        rgb.map(|v| (y + (v - y) * factor).clamp(0., 1.))
    }
}

struct Curves {
    gains: [f32; 3],
    rolloff: f32,
    contrast: f32,
    highlights: f32,
    shadows: f32,
}

impl Curves {
    fn channel(&self, c: usize, v: f32) -> f32 {
        // White balance and exposure act on light, so work in linear.
        let linear = decode(v) * self.gains[c];
        let clipped = linear.clamp(0., 1.);
        let v = encode(clipped + (shoulder(linear) - clipped) * self.rolloff);
        tone(v, self.contrast, self.highlights, self.shadows)
    }
}

fn luma(rgb: [f32; 3]) -> f32 {
    0.2126 * rgb[0] + 0.7152 * rgb[1] + 0.0722 * rgb[2]
}

/// sRGB transfer functions.
pub fn decode(v: f32) -> f32 {
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

pub fn encode(v: f32) -> f32 {
    if v <= 0.003_130_8 {
        v * 12.92
    } else {
        1.055 * v.powf(1. / 2.4) - 0.055
    }
}

/// Identity below 0.8, then rolls off smoothly so over-range values approach 1.
fn shoulder(v: f32) -> f32 {
    const KNEE: f32 = 0.8;
    if v <= KNEE {
        v.max(0.)
    } else {
        let over = v - KNEE;
        let room = 1. - KNEE;
        KNEE + room * over / (over + room)
    }
}

/// Endpoint-preserving, monotonic tone curve.
fn tone(x: f32, contrast: f32, highlights: f32, shadows: f32) -> f32 {
    let x = x.clamp(0., 1.);
    let smooth = x * x * (3. - 2. * x);
    let x = if contrast >= 0. {
        x + (smooth - x) * contrast
    } else {
        x + (x - smooth) * -contrast
    };
    let x = x + shadows * x * (1. - x) * (1. - x);
    let x = x + highlights * x * x * (1. - x);
    x.clamp(0., 1.)
}

/// A baked RGB lattice, red varying fastest (the `.cube` order).
#[derive(Clone, Debug, PartialEq)]
pub struct Lattice {
    pub size: usize,
    pub data: Vec<[f32; 3]>,
}

impl Lattice {
    pub fn identity(size: usize) -> Self {
        bake(&Grade::default(), None, 0., size)
    }

    /// Trilinear sample, as the GPU's filtered 3D texture fetch does.
    pub fn sample(&self, rgb: [f32; 3]) -> [f32; 3] {
        trilinear(&self.data, self.size, rgb)
    }
}

/// Bake the grade, followed by `cube` mixed at `strength`, into a lattice.
///
/// The per-channel curves are tabulated once per axis, so a bake costs
/// a few hundred transfer-function evaluations rather than one per node.
pub fn bake(grade: &Grade, cube: Option<&Cube>, strength: f32, size: usize) -> Lattice {
    let size = size.max(2);
    let scale = 1. / (size - 1) as f32;
    let curves = grade.curves();
    let tables: [Vec<f32>; 3] = std::array::from_fn(|c| {
        (0..size)
            .map(|i| curves.channel(c, i as f32 * scale))
            .collect()
    });
    let cube = cube.filter(|_| strength > 0.);
    let mut data = Vec::with_capacity(size * size * size);
    for b in 0..size {
        for g in 0..size {
            for r in 0..size {
                let graded = grade.mix([tables[0][r], tables[1][g], tables[2][b]]);
                let out = match cube {
                    Some(cube) => {
                        let looked = cube.sample(graded);
                        std::array::from_fn(|i| graded[i] + (looked[i] - graded[i]) * strength)
                    }
                    None => graded,
                };
                data.push(out.map(|v| v.clamp(0., 1.)));
            }
        }
    }
    Lattice { size, data }
}

pub(crate) fn trilinear(data: &[[f32; 3]], size: usize, rgb: [f32; 3]) -> [f32; 3] {
    let max = (size - 1) as f32;
    let position = rgb.map(|v| v.clamp(0., 1.) * max);
    let low = position.map(|v| (v.floor() as usize).min(size - 2));
    let frac: [f32; 3] = std::array::from_fn(|i| position[i] - low[i] as f32);
    let at = |r: usize, g: usize, b: usize| data[(b * size + g) * size + r];
    let mut out = [0.; 3];
    for corner in 0..8 {
        let (dr, dg, db) = (corner & 1, (corner >> 1) & 1, (corner >> 2) & 1);
        let weight = (if dr == 1 { frac[0] } else { 1. - frac[0] })
            * (if dg == 1 { frac[1] } else { 1. - frac[1] })
            * (if db == 1 { frac[2] } else { 1. - frac[2] });
        if weight == 0. {
            continue;
        }
        let value = at(low[0] + dr, low[1] + dg, low[2] + db);
        for i in 0..3 {
            out[i] += value[i] * weight;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: [f32; 3], b: [f32; 3], tolerance: f32) -> bool {
        a.iter().zip(b).all(|(x, y)| (x - y).abs() <= tolerance)
    }

    #[test]
    fn neutral_grade_is_identity() {
        let grade = Grade::default();
        for rgb in [
            [0., 0., 0.],
            [1., 1., 1.],
            [0.2, 0.5, 0.9],
            [0.73, 0.1, 0.33],
        ] {
            assert!(close(grade.apply(rgb), rgb, 1e-4), "{rgb:?}");
        }
        let lattice = Lattice::identity(BAKE_SIZE);
        assert!(close(
            lattice.sample([0.31, 0.62, 0.07]),
            [0.31, 0.62, 0.07],
            1e-5
        ));
    }

    #[test]
    fn exposure_is_monotonic_and_bounded() {
        let mut previous = -1.;
        for step in -4..=4 {
            let grade = Grade {
                exposure: step as f32 * 0.5,
                ..Default::default()
            };
            let value = grade.apply([0.4, 0.4, 0.4])[0];
            assert!(value > previous && value <= 1.);
            previous = value;
        }
    }

    #[test]
    fn saturation_minus_one_is_grey_and_warmth_warms() {
        let grey = Grade {
            saturation: -1.,
            ..Default::default()
        }
        .apply([0.8, 0.3, 0.1]);
        assert!((grey[0] - grey[1]).abs() < 1e-5 && (grey[1] - grey[2]).abs() < 1e-5);
        let warm = Grade {
            warmth: 1.,
            ..Default::default()
        }
        .apply([0.5, 0.5, 0.5]);
        assert!(warm[0] > warm[1] && warm[1] > warm[2]);
    }

    #[test]
    fn tone_curve_preserves_endpoints_and_order() {
        for (c, h, s) in [(1., 1., 1.), (-1., -1., -1.), (0.5, -0.4, 0.8)] {
            assert!(tone(0., c, h, s).abs() < 1e-6 && (tone(1., c, h, s) - 1.).abs() < 1e-6);
            let mut last = -1.;
            for i in 0..=100 {
                let y = tone(i as f32 / 100., c, h, s);
                assert!(y >= last - 1e-6, "c={c} h={h} s={s} at {i}");
                last = y;
            }
        }
    }

    #[test]
    fn cube_mixes_at_strength() {
        let invert = Cube::from_fn(17, |rgb| rgb.map(|v| 1. - v));
        let half = bake(&Grade::default(), Some(&invert), 0.5, 17);
        assert!(close(half.sample([0.2, 0.9, 0.4]), [0.5, 0.5, 0.5], 1e-4));
        let full = bake(&Grade::default(), Some(&invert), 1., 17);
        assert!(close(full.sample([0.2, 0.9, 0.4]), [0.8, 0.1, 0.6], 1e-4));
    }

    #[test]
    fn bake_matches_direct_application() {
        let grade = Grade {
            exposure: 0.7,
            contrast: 0.3,
            highlights: -0.4,
            shadows: 0.5,
            warmth: 0.6,
            tint: -0.2,
            vibrance: 0.4,
            saturation: -0.1,
        };
        let lattice = bake(&grade, None, 0., 9);
        let node = |i: usize| i as f32 / 8.;
        for (r, g, b) in [(0, 0, 0), (8, 8, 8), (2, 5, 7), (6, 1, 3)] {
            let baked = lattice.data[(b * 9 + g) * 9 + r];
            assert!(close(baked, grade.apply([node(r), node(g), node(b)]), 1e-5));
        }
    }

    #[test]
    fn bake_is_fast_enough_for_live_sliders() {
        let grade = Grade {
            exposure: 0.4,
            warmth: 0.3,
            vibrance: 0.2,
            ..Default::default()
        };
        let cube = Cube::from_fn(33, |rgb| rgb.map(|v| v * 0.9));
        bake(&grade, Some(&cube), 0.8, BAKE_SIZE);
        let started = std::time::Instant::now();
        for _ in 0..10 {
            bake(&grade, Some(&cube), 0.8, BAKE_SIZE);
        }
        // Generous bound so slow CI machines pass; typically well under 2 ms.
        assert!(
            started.elapsed().as_millis() < 300,
            "{:?}",
            started.elapsed()
        );
    }

    #[test]
    fn set_snaps_and_clamps() {
        let mut grade = Grade::default();
        grade.set(Control::Exposure, 0.333);
        assert_eq!(grade.exposure, 0.35);
        grade.set(Control::Tint, 7.);
        assert_eq!(grade.tint, 1.);
        assert_eq!(Control::Tint.format(0.25), "+25");
        assert_eq!(Control::Exposure.format(-0.5), "-0.50 EV");
        assert_eq!(Control::Exposure.format(-0.), "+0.00 EV");
    }
}
