//! Ready-made camera looks: grade presets (settings for the grade sliders)
//! and a starter set of creative LUTs that omalive generates itself, so
//! they carry no third-party licence.
use crate::{cube::Cube, grade::Grade};
use anyhow::{Context, Result};
use std::path::Path;

/// A named set of grade slider values.
#[derive(Clone, Copy, Debug)]
pub struct GradePreset {
    pub name: &'static str,
    pub description: &'static str,
    pub grade: Grade,
}

/// Exposure, contrast, highlights, shadows, warmth, tint, vibrance,
/// saturation.
const fn grade(
    [
        exposure,
        contrast,
        highlights,
        shadows,
        warmth,
        tint,
        vibrance,
        saturation,
    ]: [f32; 8],
) -> Grade {
    Grade {
        exposure,
        contrast,
        highlights,
        shadows,
        warmth,
        tint,
        vibrance,
        saturation,
    }
}

/// Starting points for a webcam or a camera on a flat picture style. Each
/// stays gentle: a LUT on top supplies the strong looks.
pub const GRADE_PRESETS: [GradePreset; 8] = [
    GradePreset {
        name: "Neutral",
        description: "No correction",
        grade: grade([0., 0., 0., 0., 0., 0., 0., 0.]),
    },
    GradePreset {
        name: "Bright",
        description: "Lifted and clean, for a dim room",
        grade: grade([0.3, 0.08, -0.25, 0.2, 0.05, 0., 0.15, 0.]),
    },
    GradePreset {
        name: "Punchy",
        description: "Contrast and colour back into a flat picture style",
        grade: grade([0., 0.25, -0.1, -0.05, 0., 0., 0.3, 0.05]),
    },
    GradePreset {
        name: "Warm",
        description: "Golden skin, softer highlights",
        grade: grade([0., 0.06, -0.1, 0., 0.25, 0.04, 0.1, 0.]),
    },
    GradePreset {
        name: "Cool",
        description: "Crisp daylight",
        grade: grade([0., 0.1, 0., 0., -0.2, -0.02, 0., -0.05]),
    },
    GradePreset {
        name: "Soft",
        description: "Low contrast, flattering skin",
        grade: grade([0., -0.15, -0.2, 0.15, 0.08, 0., 0., -0.08]),
    },
    GradePreset {
        name: "Low light",
        description: "More exposure, protected highlights, calmer noise",
        grade: grade([0.6, 0.05, -0.3, 0.3, 0., 0., 0., -0.1]),
    },
    GradePreset {
        name: "Moody",
        description: "Darker and desaturated",
        grade: grade([-0.15, 0.3, -0.05, -0.15, -0.05, 0., 0., -0.25]),
    },
];

/// The preset these grade values match, if any.
pub fn grade_preset(grade: &Grade) -> Option<&'static GradePreset> {
    GRADE_PRESETS.iter().find(|preset| preset.grade == *grade)
}

/// A creative LUT omalive can write into the library.
pub struct StarterLook {
    /// File stem; the library shows it with spaces.
    pub file: &'static str,
    pub look: fn([f32; 3]) -> [f32; 3],
}

pub const STARTER_LOOKS: [StarterLook; 9] = [
    StarterLook {
        file: "Warm_film",
        look: warm_film,
    },
    StarterLook {
        file: "Teal_and_orange",
        look: teal_and_orange,
    },
    StarterLook {
        file: "Clean_day",
        look: clean_day,
    },
    StarterLook {
        file: "Golden_hour",
        look: golden_hour,
    },
    StarterLook {
        file: "Soft_portrait",
        look: soft_portrait,
    },
    StarterLook {
        file: "Faded_matte",
        look: faded_matte,
    },
    StarterLook {
        file: "Cool_nordic",
        look: cool_nordic,
    },
    StarterLook {
        file: "Bleach_bypass",
        look: bleach_bypass,
    },
    StarterLook {
        file: "Mono",
        look: mono,
    },
];

/// Write any starter looks missing from `dir` (never overwriting a file),
/// returning how many were added.
pub fn install_starter_looks(dir: &Path) -> Result<usize> {
    std::fs::create_dir_all(dir).with_context(|| format!("Creating {}", dir.display()))?;
    let mut added = 0;
    for starter in &STARTER_LOOKS {
        let path = dir.join(format!("{}.cube", starter.file));
        if path.exists() {
            continue;
        }
        let mut cube = Cube::from_fn(33, starter.look);
        cube.title = starter.file.replace('_', " ");
        std::fs::write(&path, cube.to_text())
            .with_context(|| format!("Writing {}", path.display()))?;
        added += 1;
    }
    Ok(added)
}

// ---- the looks: display-referred RGB in, out, 0…1 ----

fn luma([r, g, b]: [f32; 3]) -> f32 {
    0.2126 * r + 0.7152 * g + 0.0722 * b
}

fn clamp(rgb: [f32; 3]) -> [f32; 3] {
    rgb.map(|v| v.clamp(0., 1.))
}

/// Saturation around luma: 1 keeps, 0 is grey.
fn saturate(rgb: [f32; 3], amount: f32) -> [f32; 3] {
    let y = luma(rgb);
    rgb.map(|v| y + (v - y) * amount)
}

/// A filmic S-curve: `amount` 0 is linear, 1 is strong.
fn s_curve(v: f32, amount: f32) -> f32 {
    let s = v * v * (3. - 2. * v);
    v + (s - v) * amount
}

/// Lift the blacks to `black` and lower the whites to `white`.
fn range(v: f32, black: f32, white: f32) -> f32 {
    black + v * (white - black)
}

/// Tint by `shadows` below mid-grey and `highlights` above, as offsets.
fn split(rgb: [f32; 3], shadows: [f32; 3], highlights: [f32; 3]) -> [f32; 3] {
    let y = luma(rgb).clamp(0., 1.);
    let (low, high) = ((1. - y).powi(2), y * y);
    std::array::from_fn(|i| rgb[i] + shadows[i] * low + highlights[i] * high)
}

fn warm_film(rgb: [f32; 3]) -> [f32; 3] {
    let rgb = rgb.map(|v| range(s_curve(v, 0.35), 0.03, 0.97));
    let rgb = split(rgb, [-0.01, 0.015, 0.0], [0.04, 0.01, -0.05]);
    clamp(saturate(rgb, 0.92))
}

fn teal_and_orange(rgb: [f32; 3]) -> [f32; 3] {
    let rgb = rgb.map(|v| s_curve(v, 0.3));
    let rgb = split(rgb, [-0.06, 0.02, 0.07], [0.06, 0.01, -0.07]);
    clamp(saturate(rgb, 1.05))
}

fn clean_day(rgb: [f32; 3]) -> [f32; 3] {
    let rgb = rgb.map(|v| s_curve(v, 0.2) * 1.02);
    clamp(saturate(split(rgb, [0., 0., 0.01], [0.01, 0.01, 0.]), 1.08))
}

fn golden_hour(rgb: [f32; 3]) -> [f32; 3] {
    let [r, g, b] = rgb;
    let rgb = [r * 1.07 + 0.02, g * 1.01 + 0.005, b * 0.85];
    let rgb = split(rgb, [0.01, 0., -0.02], [0.05, 0.02, -0.04]);
    clamp(saturate(rgb.map(|v| s_curve(v, 0.2)), 1.1))
}

fn soft_portrait(rgb: [f32; 3]) -> [f32; 3] {
    let rgb = rgb.map(|v| range(s_curve(v, -0.15), 0.025, 0.98));
    let rgb = split(rgb, [0.01, 0., 0.015], [0.03, 0.012, -0.01]);
    clamp(saturate(rgb, 0.9))
}

fn faded_matte(rgb: [f32; 3]) -> [f32; 3] {
    let rgb = rgb.map(|v| range(s_curve(v, 0.25), 0.08, 0.94));
    let rgb = split(rgb, [-0.01, 0.01, 0.03], [0.02, 0.01, -0.01]);
    clamp(saturate(rgb, 0.8))
}

fn cool_nordic(rgb: [f32; 3]) -> [f32; 3] {
    let [r, g, b] = rgb;
    let rgb = [r * 0.95, g * 0.99, b * 1.06 + 0.01];
    let rgb = split(
        rgb.map(|v| range(v, 0.02, 1.)),
        [0., 0.01, 0.03],
        [0., 0., 0.01],
    );
    clamp(saturate(rgb.map(|v| s_curve(v, 0.15)), 0.85))
}

fn bleach_bypass(rgb: [f32; 3]) -> [f32; 3] {
    let rgb = saturate(rgb, 0.5);
    clamp(rgb.map(|v| s_curve(v, 0.6)))
}

fn mono(rgb: [f32; 3]) -> [f32; 3] {
    let y = s_curve(luma(rgb).clamp(0., 1.), 0.3);
    [y; 3]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_are_valid_and_found_by_value() {
        for preset in &GRADE_PRESETS {
            preset.grade.validate().unwrap();
            assert_eq!(grade_preset(&preset.grade).unwrap().name, preset.name);
        }
        assert_eq!(grade_preset(&Grade::default()).unwrap().name, "Neutral");
        let custom = Grade {
            exposure: 0.05,
            ..Grade::default()
        };
        assert!(grade_preset(&custom).is_none());
    }

    #[test]
    fn looks_stay_in_range_and_keep_grey_ordered() {
        for starter in &STARTER_LOOKS {
            let mut previous = -1.;
            for i in 0..=16 {
                let v = i as f32 / 16.;
                let out = (starter.look)([v, v, v]);
                assert!(
                    out.iter().all(|c| (0. ..=1.).contains(c)),
                    "{}",
                    starter.file
                );
                // Brighter in, brighter out: no look inverts the tones.
                let y = luma(out);
                assert!(y >= previous - 1e-4, "{} at {v}", starter.file);
                previous = y;
            }
        }
    }

    #[test]
    fn starter_looks_install_once_and_never_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("Mono.cube"), "mine").unwrap();
        assert_eq!(
            install_starter_looks(dir.path()).unwrap(),
            STARTER_LOOKS.len() - 1
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("Mono.cube")).unwrap(),
            "mine"
        );
        assert_eq!(install_starter_looks(dir.path()).unwrap(), 0);
        let cube = Cube::read(&dir.path().join("Warm_film.cube")).unwrap();
        assert_eq!(cube.size, 33);
        assert_eq!(cube.title, "Warm film");
    }
}
