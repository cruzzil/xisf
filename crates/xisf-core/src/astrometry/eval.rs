//! Evaluating a solution: image coordinates to the sky and back.
//!
//! The pipeline is the composition the specification gives:
//!
//! ```text
//! image -> [image-plane step] -> projection plane -> native -> celestial
//! ```
//!
//! Only the image-plane step depends on which layers are present. Layer 1
//! gives it as a linear transformation about the reference point; layer 2
//! replaces that with a projective transformation; layer 3 adds a residual
//! field to layer 2. Each is a better approximation of the same map, which is
//! why dropping the top layer leaves a usable solution rather than a broken
//! one.
//!
//! Both directions are stored, and "decoders shall not invert a stored
//! transformation numerically" -- so the inverse pipeline uses the
//! `ProjectionToImage` matrices and splines rather than inverting the forward
//! ones.

use alloc::vec::Vec;

use super::projection::{Rotation, deproject, project};
use super::{Direction, DistortionDirection, Solution};

/// Apply a 3x3 projective transformation to a point, equation \[24\].
///
/// `(u', v', w')^T = P (x, y, 1)^T`, then `(u, v) = (u'/w', v'/w')`. A `w'`
/// of zero is a point the transformation sends to infinity, which has no
/// image.
pub(super) fn projective(matrix: &[[f64; 3]; 3], point: [f64; 2]) -> Option<[f64; 2]> {
    let [x, y] = point;
    let u = matrix[0][0] * x + matrix[0][1] * y + matrix[0][2];
    let v = matrix[1][0] * x + matrix[1][1] * y + matrix[1][2];
    let w = matrix[2][0] * x + matrix[2][1] * y + matrix[2][2];
    if w == 0.0 || !w.is_finite() {
        return None;
    }
    Some([u / w, v / w])
}

impl DistortionDirection {
    /// The residual field at a point, equation \[25\].
    ///
    /// `R_D(p) = sum_i w_i(p) S_i(p) / sum_i w_i(p)`, over whichever kinds of
    /// term the direction carries.
    pub fn residual(&self, point: [f64; 2], scratch: &mut Vec<f64>) -> [f64; 2] {
        let mut weighted = [0.0f64; 2];
        let mut total = 0.0f64;

        // A Global term has weight 1 everywhere.
        if let Some(term) = &self.global {
            let value = term.eval(point, self, scratch);
            weighted[0] += value[0];
            weighted[1] += value[1];
            total += 1.0;
        }

        // Local terms: the Wendland weight over their support disc.
        let mut local_coverage = 0.0f64;
        let mut nearest: Option<(f64, [f64; 2])> = None;
        for term in &self.local {
            let dx = point[0] - term.center[0];
            let dy = point[1] - term.center[1];
            let t = dx.hypot(dy) / term.radius;
            let weight = super::spline::wendland(t);
            local_coverage += weight;

            // Kept for the zero-coverage rule below.
            if nearest.is_none_or(|(best, _)| t < best) {
                nearest = Some((t, term.spline.eval(point, self, scratch)));
            }
            if weight > 0.0 {
                let value = term.spline.eval(point, self, scratch);
                weighted[0] += weight * value[0];
                weighted[1] += weight * value[1];
                total += weight;
            }
        }

        // The Fallback term's weight is W(s/t0): it contributes nothing where
        // the Local coverage is good and takes over where it fades.
        if let Some(fallback) = &self.fallback {
            let weight = super::spline::wendland(local_coverage / fallback.threshold);
            if weight > 0.0 {
                let value = fallback.spline.eval(point, self, scratch);
                weighted[0] += weight * value[0];
                weighted[1] += weight * value[1];
                total += weight;
            }
        }

        if total > 0.0 {
            return [weighted[0] / total, weighted[1] / total];
        }

        // "If the sum of weights is zero at a point p, which can only happen
        // in a direction without a Fallback term, R_D(p) shall be the value of
        // the Local term with the smallest t, or zero if there are no terms."
        match nearest {
            Some((_, value)) => value,
            None => [0.0, 0.0],
        }
    }
}

impl Solution {
    /// The projection-plane coordinates of an image point, in degrees.
    ///
    /// This is the image-plane step, using the highest layer available.
    fn image_to_plane(&self, image: [f64; 2], scratch: &mut Vec<f64>) -> Option<[f64; 2]> {
        // Layer 3, when it is present and usable.
        if let Some(distortion) = &self.distortion {
            let projective_matrix = &self.projective.as_ref()?.image_to_projection;
            let base = projective(projective_matrix, image)?;
            let residual = distortion.image_to_projection.residual(image, scratch);
            return Some([base[0] + residual[0], base[1] + residual[1]]);
        }
        // Layer 2.
        if let Some(transformation) = &self.projective {
            return projective(&transformation.image_to_projection, image);
        }
        // Layer 1: the linear transformation about the reference point.
        let [x0, y0] = self.projection.reference_image;
        let dx = image[0] - x0;
        let dy = image[1] - y0;
        let m = &self.projection.linear_transformation;
        Some([m[0][0] * dx + m[0][1] * dy, m[1][0] * dx + m[1][1] * dy])
    }

    /// The image coordinates of a projection-plane point.
    fn plane_to_image(&self, plane: [f64; 2], scratch: &mut Vec<f64>) -> Option<[f64; 2]> {
        if let Some(distortion) = &self.distortion {
            let projective_matrix = &self.projective.as_ref()?.projection_to_image;
            let base = projective(projective_matrix, plane)?;
            let residual = distortion.projection_to_image.residual(plane, scratch);
            return Some([base[0] + residual[0], base[1] + residual[1]]);
        }
        if let Some(transformation) = &self.projective {
            return projective(&transformation.projection_to_image, plane);
        }
        // Layer 1: invert the 2x2 linear transformation. This is not the
        // "shall not invert a stored transformation numerically" case -- that
        // forbids inverting the *projective* and distortion layers, which are
        // separate fits per direction. Layer 1 stores one matrix and the
        // inverse of a 2x2 is exact arithmetic, not a numerical inversion.
        let m = &self.projection.linear_transformation;
        let det = m[0][0] * m[1][1] - m[0][1] * m[1][0];
        if det == 0.0 || !det.is_finite() {
            return None;
        }
        let [u, v] = plane;
        let dx = (m[1][1] * u - m[0][1] * v) / det;
        let dy = (-m[1][0] * u + m[0][0] * v) / det;
        let [x0, y0] = self.projection.reference_image;
        Some([x0 + dx, y0 + dy])
    }

    /// The rotation between native and celestial coordinates, from layer 1.
    fn rotation(&self) -> Option<Rotation> {
        Rotation::new(
            self.projection.reference_celestial,
            self.projection
                .reference_native
                .unwrap_or_else(|| self.projection.system.default_reference_native()),
            self.projection.celestial_pole_native,
        )
    }

    /// Celestial coordinates of an image point: right ascension in `[0, 360)`
    /// and declination, both in degrees.
    ///
    /// Image coordinates are the specification's: "the pixel with column index
    /// i and row index j covers the region [i, i+1] x [j, j+1], so its center
    /// is at (i + 0.5, j + 0.5)".
    ///
    /// `None` for a point the solution does not map -- outside the projection's
    /// domain, or with parameters that define no rotation.
    pub fn image_to_celestial(&self, image: [f64; 2]) -> Option<[f64; 2]> {
        let mut scratch = Vec::new();
        let plane = self.image_to_plane(image, &mut scratch)?;
        let native = deproject(plane, self.projection.system)?;
        Some(super::native_to_celestial(native, self.rotation()?))
    }

    /// Image coordinates of a celestial position. The inverse of
    /// [`Solution::image_to_celestial`], by the stored inverse rather than by
    /// inverting the forward transformation.
    pub fn celestial_to_image(&self, celestial: [f64; 2]) -> Option<[f64; 2]> {
        let mut scratch = Vec::new();
        let native = super::celestial_to_native(celestial, self.rotation()?);
        let plane = project(native, self.projection.system)?;
        self.plane_to_image(plane, &mut scratch)
    }

    /// Which direction's distortion model applies to a step, for callers that
    /// want the residual field on its own.
    pub fn distortion_for(&self, direction: Direction) -> Option<&DistortionDirection> {
        let model = self.distortion.as_ref()?;
        Some(match direction {
            Direction::ImageToProjection => &model.image_to_projection,
            Direction::ProjectionToImage => &model.projection_to_image,
        })
    }
}
