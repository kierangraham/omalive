//! Y'CbCr ↔ R'G'B' conversion shared by the GPU shader and the CPU fallback.

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct YuvMatrix {
    pub kr: f32,
    pub kb: f32,
    /// Full-range (0…255) rather than studio-swing (16…235 / 16…240) coding.
    pub full_range: bool,
}

impl YuvMatrix {
    pub const BT601: Self = Self {
        kr: 0.299,
        kb: 0.114,
        full_range: false,
    };
    pub const BT709: Self = Self {
        kr: 0.2126,
        kb: 0.0722,
        full_range: false,
    };

    pub fn full(self, full_range: bool) -> Self {
        Self { full_range, ..self }
    }

    /// (offset, luma scale, chroma scale) mapping 0…1 coded values to signal.
    fn range(self) -> (f32, f32, f32) {
        if self.full_range {
            (0., 1., 1.)
        } else {
            (16. / 255., 255. / 219., 255. / 224.)
        }
    }

    /// Decode normalised (0…1) Y'CbCr samples to R'G'B'.
    pub fn to_rgb(self, y: f32, cb: f32, cr: f32) -> [f32; 3] {
        let (offset, luma, chroma) = self.range();
        let y = (y - offset) * luma;
        let (cb, cr) = ((cb - 0.5) * chroma, (cr - 0.5) * chroma);
        let kg = 1. - self.kr - self.kb;
        let r = y + 2. * (1. - self.kr) * cr;
        let b = y + 2. * (1. - self.kb) * cb;
        let g = (y - self.kr * r - self.kb * b) / kg;
        [r, g, b].map(|v| v.clamp(0., 1.))
    }

    /// Encode R'G'B' to normalised Y'CbCr samples.
    pub fn from_rgb(self, rgb: [f32; 3]) -> [f32; 3] {
        let (offset, luma, chroma) = self.range();
        let kg = 1. - self.kr - self.kb;
        let y = self.kr * rgb[0] + kg * rgb[1] + self.kb * rgb[2];
        let cb = (rgb[2] - y) / (2. * (1. - self.kb));
        let cr = (rgb[0] - y) / (2. * (1. - self.kr));
        [y / luma + offset, cb / chroma + 0.5, cr / chroma + 0.5]
    }

    /// Row-major 3×4 matrix (including offsets) for the shader:
    /// `rgb = M · (y, cb, cr, 1)`.
    pub fn decode_matrix(self) -> [[f32; 4]; 3] {
        let (offset, luma, chroma) = self.range();
        let kg = 1. - self.kr - self.kb;
        let cr_r = 2. * (1. - self.kr) * chroma;
        let cb_b = 2. * (1. - self.kb) * chroma;
        let cb_g = -self.kb * cb_b / kg;
        let cr_g = -self.kr * cr_r / kg;
        let row = |cb: f32, cr: f32| [luma, cb, cr, -offset * luma - 0.5 * (cb + cr)];
        [row(0., cr_r), row(cb_g, cr_g), row(cb_b, 0.)]
    }

    /// Row-major 3×4 matrix: `(y, cb, cr) = M · (r, g, b, 1)`.
    pub fn encode_matrix(self) -> [[f32; 4]; 3] {
        let (offset, luma, chroma) = self.range();
        let kg = 1. - self.kr - self.kb;
        let y = [self.kr / luma, kg / luma, self.kb / luma, offset];
        let cb_scale = 1. / (2. * (1. - self.kb) * chroma);
        let cr_scale = 1. / (2. * (1. - self.kr) * chroma);
        let cb = [
            -self.kr * cb_scale,
            -kg * cb_scale,
            (1. - self.kb) * cb_scale,
            0.5,
        ];
        let cr = [
            (1. - self.kr) * cr_scale,
            -kg * cr_scale,
            -self.kb * cr_scale,
            0.5,
        ];
        [y, cb, cr]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apply(m: [[f32; 4]; 3], v: [f32; 3]) -> [f32; 3] {
        m.map(|row| row[0] * v[0] + row[1] * v[1] + row[2] * v[2] + row[3])
    }

    #[test]
    fn round_trips_and_matrices_agree() {
        for matrix in [
            YuvMatrix::BT601,
            YuvMatrix::BT709,
            YuvMatrix::BT601.full(true),
        ] {
            for rgb in [[0., 0., 0.], [1., 1., 1.], [0.8, 0.2, 0.4], [0.1, 0.9, 0.3]] {
                let yuv = matrix.from_rgb(rgb);
                let back = matrix.to_rgb(yuv[0], yuv[1], yuv[2]);
                assert!(
                    back.iter().zip(rgb).all(|(a, b)| (a - b).abs() < 1e-4),
                    "{matrix:?} {rgb:?}"
                );
                let shader = apply(matrix.decode_matrix(), yuv);
                assert!(shader.iter().zip(rgb).all(|(a, b)| (a - b).abs() < 1e-4));
                let encoded = apply(matrix.encode_matrix(), rgb);
                assert!(encoded.iter().zip(yuv).all(|(a, b)| (a - b).abs() < 1e-5));
            }
        }
    }

    #[test]
    fn studio_swing_black_and_white() {
        let yuv = YuvMatrix::BT601.from_rgb([1., 1., 1.]);
        assert!((yuv[0] * 255. - 235.).abs() < 0.01);
        let black = YuvMatrix::BT601.from_rgb([0., 0., 0.]);
        assert!((black[0] * 255. - 16.).abs() < 0.01 && (black[1] - 0.5).abs() < 1e-6);
    }
}
