//! Colour space transformations: Annex B of the specification.
//!
//! Annex B is **normative**, unlike Annex A. It defines the transformations
//! between RGB, CIE XYZ and CIE L*a*b*, and the computation of colorimetrically
//! defined grayscale, all relative to the image's RGB working space and the
//! D50 reference white.
//!
//! Every nominal component here is in the `[0, 1]` range, in and out. That is
//! the format's convention rather than a convenience: "All nominal components
//! of pixels in the RGB, grayscale and CIE L*a*b* color spaces are represented
//! in the normalized \[0,1\] range. Encoders and decoders *shall* apply the
//! representable range of an image to map pixel samples to and from this
//! range, and *shall* clip the results of the transformations below to the
//! \[0,1\] range."
//!
//! Mapping an image's samples onto `[0, 1]` is the caller's job -- it depends
//! on the sample format and the image's `bounds` -- but the clipping is done
//! here, because it is part of the transformation rather than of the caller's
//! bookkeeping.
//!
//! The L*a*b* components are normalized too, which is what lets a CIE L*a*b*
//! image be stored with an integer sample format: `L*` in `[0, 100]` becomes
//! `L` in `[0, 1]`, and the signed `a*` and `b*` are shifted so that the
//! achromatic axis sits at one half rather than at zero.

use crate::image::{D50_WHITE, Gamma, RgbWorkingSpace};

/// `epsilon = 216/24389`, equation \[57\].
const EPSILON: f64 = 216.0 / 24389.0;
/// `kappa = 24389/27`, equation \[57\].
const KAPPA: f64 = 24389.0 / 27.0;

/// Clip to the nominal range, which the annex requires of every result.
fn clip(x: f64) -> f64 {
    if x.is_nan() { 0.0 } else { x.clamp(0.0, 1.0) }
}

/// The sRGB linearization function `S(x)`, equation \[49\].
///
/// Note that this is *not* a pure exponent, which is why a working space whose
/// gamma is the word `sRGB` cannot be handled by raising to a power: the
/// piecewise form has a linear segment near black that a pure 2.2 or 2.4
/// exponent does not.
pub fn srgb_linearize(x: f64) -> f64 {
    if x <= 0.04045 { x / 12.92 } else { ((x + 0.055) / 1.055).powf(2.4) }
}

/// The sRGB delinearization function `T(x)`, equation \[55\]. The inverse of
/// [`srgb_linearize`].
pub fn srgb_delinearize(x: f64) -> f64 {
    if x <= 0.0031308 { 12.92 * x } else { 1.055 * x.powf(1.0 / 2.4) - 0.055 }
}

/// `f(t)`, equation \[56\]: the companding function of the L*a*b* transform.
fn f(t: f64) -> f64 {
    if t > EPSILON { t.cbrt() } else { (KAPPA * t + 16.0) / 116.0 }
}

/// `g(t)`, equation \[61\]: the inverse of [`f`].
fn g(t: f64) -> f64 {
    let cube = t * t * t;
    if cube > EPSILON { cube } else { (116.0 * t - 16.0) / KAPPA }
}

/// The transformations for one RGB working space.
///
/// Built once and reused, because the RGB-to-XYZ matrix and its inverse depend
/// only on the working space, and an image has one.
#[derive(Clone, PartialEq, Debug)]
pub struct ColorTransform {
    gamma: Gamma,
    /// Equation \[51\].
    to_xyz: [[f64; 3]; 3],
    /// Its inverse, for equation \[53\].
    from_xyz: [[f64; 3]; 3],
}

impl ColorTransform {
    /// The transformations for a working space.
    ///
    /// `None` when the space's primaries are degenerate, which is the same
    /// condition that makes its luminance coefficients underivable.
    pub fn new(space: &RgbWorkingSpace) -> Option<Self> {
        let [x, y, luminance] = [space.x, space.y, space.luminance];
        let mut to_xyz = [[0.0f64; 3]; 3];
        for i in 0..3 {
            if y[i] == 0.0 {
                return None;
            }
            // Column i is the primary's tristimulus values scaled by its
            // luminance coefficient.
            to_xyz[0][i] = luminance[i] * x[i] / y[i];
            to_xyz[1][i] = luminance[i];
            to_xyz[2][i] = luminance[i] * (1.0 - x[i] - y[i]) / y[i];
        }
        let from_xyz = invert(&to_xyz)?;
        Some(ColorTransform { gamma: space.gamma, to_xyz, from_xyz })
    }

    /// The transformations for the sRGB space, which applies when an image
    /// declares none.
    pub fn srgb() -> Self {
        Self::new(&RgbWorkingSpace::srgb()).expect("sRGB is a valid working space")
    }

    /// Linearize one nominal component: equation \[48\], or \[49\] for sRGB.
    pub fn linearize(&self, value: f64) -> f64 {
        match self.gamma {
            Gamma::Srgb => srgb_linearize(value),
            Gamma::Exponent(exponent) => value.max(0.0).powf(exponent),
        }
    }

    /// The inverse: equation \[54\], or \[55\] for sRGB.
    pub fn delinearize(&self, value: f64) -> f64 {
        match self.gamma {
            Gamma::Srgb => srgb_delinearize(value),
            Gamma::Exponent(exponent) => value.max(0.0).powf(1.0 / exponent),
        }
    }

    /// The matrix applied to already-linear components, without the
    /// linearization step. Exposed so that the matrix itself can be compared
    /// against published values for a known working space.
    pub fn rgb_to_xyz_linear(&self, linear: [f64; 3]) -> [f64; 3] {
        apply(&self.to_xyz, linear)
    }

    /// Nominal RGB to CIE XYZ tristimulus values, equations \[48\] to \[51\].
    pub fn rgb_to_xyz(&self, rgb: [f64; 3]) -> [f64; 3] {
        let linear = [self.linearize(rgb[0]), self.linearize(rgb[1]), self.linearize(rgb[2])];
        apply(&self.to_xyz, linear)
    }

    /// CIE XYZ tristimulus values to nominal RGB, equations \[53\] and \[54\].
    pub fn xyz_to_rgb(&self, xyz: [f64; 3]) -> [f64; 3] {
        let linear = apply(&self.from_xyz, xyz);
        [
            clip(self.delinearize(linear[0].max(0.0))),
            clip(self.delinearize(linear[1].max(0.0))),
            clip(self.delinearize(linear[2].max(0.0))),
        ]
    }

    /// Nominal RGB to nominal CIE L*a*b*, as XISF serializes it.
    pub fn rgb_to_lab(&self, rgb: [f64; 3]) -> [f64; 3] {
        xyz_to_lab(self.rgb_to_xyz(rgb))
    }

    /// Nominal CIE L*a*b* to nominal RGB.
    pub fn lab_to_rgb(&self, lab: [f64; 3]) -> [f64; 3] {
        self.xyz_to_rgb(lab_to_xyz(lab))
    }

    /// The colorimetric grayscale component, equation \[64\].
    ///
    /// "A colorimetrically defined grayscale component *shall* be the L
    /// component of the CIE L*a*b* space", computed from the linear RGB
    /// components through the second row of the matrix -- which is to say
    /// through the luminance coefficients, since that row *is* them.
    pub fn rgb_to_gray(&self, rgb: [f64; 3]) -> f64 {
        let linear = [self.linearize(rgb[0]), self.linearize(rgb[1]), self.linearize(rgb[2])];
        let y = self.to_xyz[1][0] * linear[0]
            + self.to_xyz[1][1] * linear[1]
            + self.to_xyz[1][2] * linear[2];
        clip(1.16 * f(y) - 0.16)
    }
}

/// CIE XYZ to nominal CIE L*a*b*, equations \[52\], \[56\] and \[60\].
///
/// The tristimulus values are normalized to the D50 reference white first,
/// which is what Revision 1 corrected: without it the components do not land
/// in the range an integer sample format can hold.
pub fn xyz_to_lab(xyz: [f64; 3]) -> [f64; 3] {
    // [52]: Y_W is 1, so Y needs no division.
    let fx = f(xyz[0] / D50_WHITE[0]);
    let fy = f(xyz[1]);
    let fz = f(xyz[2] / D50_WHITE[2]);

    // [60], which is [58] and [59] composed: L* in [0,100] becomes L in
    // [0,1], and the signed a* and b* are shifted onto [0,1] with the
    // achromatic axis at one half.
    [
        clip(1.16 * fy - 0.16),
        clip(0.5 + (29.0 / 50.0) * (fx - fy)),
        clip(0.5 + (29.0 / 50.0) * (fy - fz)),
    ]
}

/// Nominal CIE L*a*b* to CIE XYZ, equations \[61\] to \[63\].
pub fn lab_to_xyz(lab: [f64; 3]) -> [f64; 3] {
    // [62]
    let fy = (lab[0] + 0.16) / 1.16;
    let fx = fy + (50.0 / 29.0) * (lab[1] - 0.5);
    let fz = fy - (50.0 / 29.0) * (lab[2] - 0.5);
    // [63]
    [g(fx) * D50_WHITE[0], g(fy), g(fz) * D50_WHITE[2]]
}

fn apply(matrix: &[[f64; 3]; 3], v: [f64; 3]) -> [f64; 3] {
    let mut out = [0.0; 3];
    for (row, slot) in matrix.iter().zip(out.iter_mut()) {
        *slot = row[0] * v[0] + row[1] * v[1] + row[2] * v[2];
    }
    out
}

fn invert(m: &[[f64; 3]; 3]) -> Option<[[f64; 3]; 3]> {
    let det = m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
        - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
        + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
    if !det.is_finite() || det.abs() < 1e-15 {
        return None;
    }
    let mut out = [[0.0f64; 3]; 3];
    for (i, row) in out.iter_mut().enumerate() {
        for (j, slot) in row.iter_mut().enumerate() {
            // The cofactor of (j, i), which transposes as it goes.
            let (a, b) = ((j + 1) % 3, (j + 2) % 3);
            let (c, d) = ((i + 1) % 3, (i + 2) % 3);
            *slot = (m[a][c] * m[b][d] - m[a][d] * m[b][c]) / det;
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The defining property of the matrix: it must carry the linear RGB white
    /// point (1, 1, 1) onto the D50 reference white. Revision 1 derives the
    /// luminance coefficients from exactly that condition, so this is the
    /// check that the matrix and the derivation agree.
    #[test]
    fn the_matrix_carries_white_to_the_reference_white() {
        let transform = ColorTransform::srgb();
        let white = apply(&transform.to_xyz, [1.0, 1.0, 1.0]);
        for (got, want) in white.iter().zip(D50_WHITE) {
            assert!((got - want).abs() < 1e-9, "white came out {white:?}, wanted {D50_WHITE:?}");
        }
    }

    /// An independent check on the matrix, against a source outside the
    /// specification: Bruce Lindbloom's published sRGB/D50 RGB-to-XYZ matrix,
    /// which the specification itself cites for the derivation.
    ///
    /// Every other test here is self-consistency -- round trips, and inverses
    /// agreeing with their forwards -- and self-consistency cannot catch a
    /// formula that is wrong in the same way in both directions. This can.
    ///
    /// The tolerance is 1e-4 rather than something tighter because the
    /// specification gives its chromaticity coordinates to six decimal places
    /// and those are the numbers a file carries, so the residual is the
    /// precision of the input rather than error in the transformation.
    #[test]
    fn the_matrix_agrees_with_an_independent_published_one() {
        let transform = ColorTransform::srgb();
        let reference = [
            [0.4360747, 0.3850649, 0.1430804],
            [0.2225045, 0.7168786, 0.0606169],
            [0.0139322, 0.0971045, 0.7141733],
        ];
        // The columns are the images of the linear basis vectors.
        let columns = [
            transform.rgb_to_xyz_linear([1.0, 0.0, 0.0]),
            transform.rgb_to_xyz_linear([0.0, 1.0, 0.0]),
            transform.rgb_to_xyz_linear([0.0, 0.0, 1.0]),
        ];
        let mut worst = 0.0f64;
        for i in 0..3 {
            for j in 0..3 {
                worst = worst.max((columns[j][i] - reference[i][j]).abs());
            }
        }
        assert!(worst < 1e-4, "the matrix differs from the published one by {worst}");
    }

    /// Chromatic colours, against an independent implementation.
    ///
    /// Every other L*a*b* test here is invariant under swapping `a` and `b`,
    /// or under flipping the sign of the 29/50 term, so long as the forward
    /// and inverse transforms are changed together -- round trips, inverses
    /// and the achromatic white and black points all survive it untouched.
    /// A swap would make every CIE L*a*b* image this library writes green
    /// where it should be red, and reading it back through the same code
    /// would look perfect.
    ///
    /// These values come from `colour-science` 0.4.7, computed with the
    /// specification's own D50 white point and sRGB primaries and converted
    /// into XISF's normalized form. They agree with this implementation to
    /// 5e-10.
    ///
    /// ```text
    /// pip install colour-science
    /// python - <<'PY'
    /// import numpy as np, colour
    /// from colour.models import eotf_sRGB
    /// D50 = np.array([0.96422, 1.0, 0.82521])
    /// xy = np.array([[0.648431, 0.330856], [0.321152, 0.597871], [0.155886, 0.066044]])
    /// wp = colour.XYZ_to_xy(D50)
    /// M = colour.normalised_primary_matrix(xy, wp)
    /// for rgb in [(1,0,0), (0,1,0), (0,0,1), (0.2,0.6,0.35), (1,1,0)]:
    ///     L, a, b = colour.XYZ_to_Lab(M @ eotf_sRGB(np.array(rgb, float)), wp)
    ///     print(L/100, 0.5 + a*116/100000, 0.5 + b*116/40000)
    /// PY
    /// ```
    #[test]
    fn chromatic_colours_agree_with_an_independent_implementation() {
        let transform = ColorTransform::srgb();
        let reference = [
            ("red", [1.0, 0.0, 0.0], [0.542_902_841, 0.593_741_425, 0.702_671_220]),
            ("green", [0.0, 1.0, 0.0], [0.878_185_825, 0.408_031_212, 0.734_881_397]),
            ("blue", [0.0, 0.0, 1.0], [0.295_686_645, 0.579_225_348, 0.175_117_300]),
            ("mid", [0.2, 0.6, 0.35], [0.562_694_762, 0.451_611_727, 0.571_345_428]),
            ("yellow", [1.0, 1.0, 0.0], [0.976_069_495, 0.481_726_180, 0.770_835_430]),
        ];
        for (name, rgb, want) in reference {
            let got = transform.rgb_to_lab(rgb);
            for i in 0..3 {
                assert!(
                    (got[i] - want[i]).abs() < 1e-8,
                    "{name}: got {got:?}, an independent implementation gives {want:?}"
                );
            }
        }
    }

    /// The sign structure, stated on its own so that the intent survives a
    /// refactor even if the reference table above is ever regenerated: `a` is
    /// the red-green axis and `b` the blue-yellow one, with the achromatic
    /// axis at one half.
    #[test]
    fn the_chromatic_axes_point_the_way_they_should() {
        let t = ColorTransform::srgb();
        assert!(t.rgb_to_lab([1.0, 0.0, 0.0])[1] > 0.5, "red must be on the +a side");
        assert!(t.rgb_to_lab([0.0, 1.0, 0.0])[1] < 0.5, "green must be on the -a side");
        assert!(t.rgb_to_lab([0.0, 0.0, 1.0])[2] < 0.5, "blue must be on the -b side");
        assert!(t.rgb_to_lab([1.0, 1.0, 0.0])[2] > 0.5, "yellow must be on the +b side");
    }

    #[test]
    fn the_matrix_inverse_is_an_inverse() {
        let transform = ColorTransform::srgb();
        let mut product = [[0.0f64; 3]; 3];
        for (i, row) in product.iter_mut().enumerate() {
            for (j, slot) in row.iter_mut().enumerate() {
                for k in 0..3 {
                    *slot += transform.to_xyz[i][k] * transform.from_xyz[k][j];
                }
            }
        }
        for (i, row) in product.iter().enumerate() {
            for (j, got) in row.iter().enumerate() {
                let want = if i == j { 1.0 } else { 0.0 };
                assert!((got - want).abs() < 1e-12, "row {i} column {j}");
            }
        }
    }

    /// White is L = 1 on the achromatic axis, black is L = 0 on it. Those are
    /// the two points every implementation must agree on.
    #[test]
    fn white_and_black_land_where_they_should() {
        let transform = ColorTransform::srgb();

        let [l, a, b] = transform.rgb_to_lab([1.0, 1.0, 1.0]);
        assert!((l - 1.0).abs() < 1e-9, "white L = {l}");
        assert!((a - 0.5).abs() < 1e-9, "white a = {a}, the achromatic axis is at one half");
        assert!((b - 0.5).abs() < 1e-9, "white b = {b}");

        let [l, a, b] = transform.rgb_to_lab([0.0, 0.0, 0.0]);
        assert!(l.abs() < 1e-9, "black L = {l}");
        assert!((a - 0.5).abs() < 1e-9, "black a = {a}");
        assert!((b - 0.5).abs() < 1e-9, "black b = {b}");
    }

    /// The whole chain must round-trip: RGB to L*a*b* and back, for colours
    /// across the gamut including the saturated primaries.
    #[test]
    fn rgb_round_trips_through_lab() {
        let transform = ColorTransform::srgb();
        let colors = [
            [0.0, 0.0, 0.0],
            [1.0, 1.0, 1.0],
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, 0.0, 1.0],
            [0.5, 0.5, 0.5],
            [0.2, 0.7, 0.35],
            [0.94, 0.11, 0.68],
            [0.01, 0.02, 0.03],
        ];
        for rgb in colors {
            let lab = transform.rgb_to_lab(rgb);
            let back = transform.lab_to_rgb(lab);
            for i in 0..3 {
                assert!(
                    (rgb[i] - back[i]).abs() < 1e-9,
                    "{rgb:?} came back {back:?} through {lab:?}"
                );
            }
        }
    }

    /// And through XYZ on its own, which isolates the companding from the
    /// matrix.
    #[test]
    fn rgb_round_trips_through_xyz() {
        for space in [RgbWorkingSpace::srgb(), adobe_rgb()] {
            let transform = ColorTransform::new(&space).expect("valid space");
            for rgb in [[0.3, 0.6, 0.9], [1.0, 1.0, 1.0], [0.0, 0.0, 0.0], [0.77, 0.25, 0.4]] {
                let back = transform.xyz_to_rgb(transform.rgb_to_xyz(rgb));
                for i in 0..3 {
                    assert!((rgb[i] - back[i]).abs() < 1e-9, "{rgb:?} came back {back:?}");
                }
            }
        }
    }

    /// The sRGB transfer functions are inverses of each other, including
    /// across the join between their two segments.
    #[test]
    fn the_srgb_transfer_functions_invert_each_other() {
        for i in 0..=1000 {
            let x = f64::from(i) / 1000.0;
            let back = srgb_delinearize(srgb_linearize(x));
            assert!((x - back).abs() < 1e-12, "{x} came back {back}");
        }
        // The join itself: 0.04045 linearizes to 0.0031308.
        assert!((srgb_linearize(0.04045) - 0.0031308).abs() < 1e-7);
    }

    /// `f` and `g` are inverses, including across the epsilon threshold where
    /// the definition changes from a cube root to a linear segment.
    #[test]
    fn the_companding_functions_invert_each_other() {
        for i in 0..=1000 {
            let t = f64::from(i) / 1000.0;
            assert!((g(f(t)) - t).abs() < 1e-12, "t = {t}");
        }
        // Either side of epsilon.
        for t in [EPSILON * 0.5, EPSILON, EPSILON * 1.5] {
            assert!((g(f(t)) - t).abs() < 1e-12, "t = {t}");
        }
    }

    /// "A colorimetrically defined grayscale component shall be the L
    /// component of the CIE L*a*b* space" -- so for any colour, the grayscale
    /// value and the L of its L*a*b* must be the same number. They are
    /// computed by different routes, which is what makes this worth checking.
    #[test]
    fn grayscale_is_the_lightness_component() {
        let transform = ColorTransform::srgb();
        for rgb in
            [[0.0, 0.0, 0.0], [1.0, 1.0, 1.0], [1.0, 0.0, 0.0], [0.25, 0.5, 0.75], [0.9, 0.1, 0.4]]
        {
            let gray = transform.rgb_to_gray(rgb);
            let [l, _, _] = transform.rgb_to_lab(rgb);
            assert!((gray - l).abs() < 1e-12, "{rgb:?}: gray {gray}, L {l}");
        }
    }

    /// A pure exponent gamma is not the sRGB function, and using one for the
    /// other is a visible error rather than a rounding one.
    #[test]
    fn an_exponent_gamma_is_not_the_srgb_function() {
        let srgb = ColorTransform::srgb();
        let exponent = ColorTransform::new(&adobe_rgb()).expect("valid");
        let difference = (srgb.linearize(0.5) - exponent.linearize(0.5)).abs();
        assert!(difference > 1e-3, "the two transfer functions differ by only {difference}");
    }

    /// Degenerate primaries define no working space and so no transform.
    #[test]
    fn degenerate_primaries_have_no_transform() {
        let mut space = RgbWorkingSpace::srgb();
        space.x = [0.3, 0.3, 0.3];
        space.y = [0.3, 0.3, 0.3];
        assert!(ColorTransform::new(&space).is_none());
    }

    fn adobe_rgb() -> RgbWorkingSpace {
        RgbWorkingSpace::new(
            Gamma::Exponent(2.2),
            [0.648431, 0.230154, 0.155886],
            [0.330856, 0.701572, 0.066044],
            Some("Adobe RGB (1998)".into()),
        )
        .expect("valid primaries")
    }
}
