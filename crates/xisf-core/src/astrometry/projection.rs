//! Projections and spherical rotation: Annex A of the specification.
//!
//! Annex A is informative, and says so: it "restates, for the projection
//! systems supported by the AstrometricSolution namespace, the transformations
//! of the WCS formulation ... in the exact form implemented by the reference
//! implementation of this specification, so that no ambiguity remains". In
//! case of discrepancy the WCS formulation governs. What is implemented here
//! is Annex A's form, for the reason it gives: it is what the reference
//! implementation does, so it is what agreement is measured against.
//!
//! Every angle is in degrees, in and out. That is the specification's
//! convention, not a convenience -- the stored properties are in degrees, and
//! converting at the edges rather than throughout keeps the formulas
//! recognisable against the text.

use super::{ProjectionClass, ProjectionSystem};

/// `R = 180/pi`, "the radius, in degrees, of the sphere on which the
/// projections are defined".
const R: f64 = 180.0 / core::f64::consts::PI;

/// The two-argument arctangent of the conventions section: "the angle psi in
/// the range (-180, 180] such that (cos psi, sin psi) is proportional to
/// (x, y)".
///
/// Note the argument order. This is `arg(x, y)` with `x` the cosine-like
/// component, which is `atan2(y, x)` -- the opposite order to the library
/// function, and an easy way to get every rotation subtly wrong.
fn arg(x: f64, y: f64) -> f64 {
    y.atan2(x).to_degrees()
}

/// Reduce an angle to `(-180, 180]`.
fn reduce(mut angle: f64) -> f64 {
    while angle > 180.0 {
        angle -= 360.0;
    }
    while angle <= -180.0 {
        angle += 360.0;
    }
    angle
}

/// Reduce a right ascension to `[0, 360)`.
pub fn reduce_ra(mut angle: f64) -> f64 {
    angle %= 360.0;
    if angle < 0.0 {
        angle += 360.0;
    }
    angle
}

fn sin_d(degrees: f64) -> f64 {
    degrees.to_radians().sin()
}

fn cos_d(degrees: f64) -> f64 {
    degrees.to_radians().cos()
}

/// The parameters of the spherical rotation, resolved from a solution's first
/// layer with the defaults the specification prescribes.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Rotation {
    /// Celestial coordinates of the native pole.
    pub pole: [f64; 2],
    /// Native longitude of the celestial pole.
    pub phi_p: f64,
}

impl Rotation {
    /// Compute the celestial coordinates of the native pole, §12.2.1.
    ///
    /// `reference_celestial` is `(alpha0, delta0)`, `reference_native` is
    /// `(phi0, theta0)`, and `celestial_pole_native` is `(phi_p, theta_p)`
    /// where the file states it.
    pub fn new(
        reference_celestial: [f64; 2],
        reference_native: [f64; 2],
        celestial_pole_native: Option<[f64; 2]>,
    ) -> Option<Self> {
        let [alpha0, delta0] = reference_celestial;
        let [phi0, theta0] = reference_native;

        // "the default value of phi_p is phi0 if delta0 >= theta0, and
        // phi0 + 180 otherwise"; the default theta_p is 90.
        let (phi_p, theta_p) = match celestial_pole_native {
            Some([phi_p, theta_p]) => (phi_p, theta_p),
            None => (reduce(if delta0 >= theta0 { phi0 } else { phi0 + 180.0 }), 90.0),
        };

        // [29]: if theta0 == 90 the reference point *is* the native pole.
        if (theta0 - 90.0).abs() < f64::EPSILON {
            return Some(Rotation { pole: [alpha0, delta0], phi_p });
        }

        // [30]
        let z =
            (cos_d(theta0).powi(2) * cos_d(phi_p - phi0).powi(2) + sin_d(theta0).powi(2)).sqrt();

        let delta_p = if z == 0.0 {
            // "which happens when theta0 = 0 and |phi_p - phi0| = 90, the
            // parameters are only valid when delta0 = 0, and then
            // delta_p = theta_p (clipped to [-90, 90])".
            if delta0 != 0.0 {
                return None;
            }
            theta_p.clamp(-90.0, 90.0)
        } else {
            let ratio = sin_d(delta0) / z;
            // "The parameters are invalid when |sin delta0| > z."
            if ratio.abs() > 1.0 {
                return None;
            }
            // [31]: two candidates.
            let base = arg(cos_d(theta0) * cos_d(phi_p - phi0), sin_d(theta0));
            let offset = ratio.acos().to_degrees();
            let candidates = [reduce(base + offset), reduce(base - offset)];

            let in_range: Vec<f64> =
                candidates.iter().copied().filter(|d| (-90.0..=90.0).contains(d)).collect();
            match in_range.len() {
                0 => return None,
                // "If both candidates are in the range, delta_p is the one
                // closest to theta_p."
                _ => *in_range
                    .iter()
                    .min_by(|a, b| (*a - theta_p).abs().total_cmp(&(*b - theta_p).abs()))
                    .expect("non-empty"),
            }
        };

        // [32] and [33]: the celestial longitude of the native pole.
        let alpha_p = if (cos_d(delta_p) * cos_d(delta0)).abs() < 1e-12 {
            if cos_d(delta0).abs() < 1e-12 {
                alpha0
            } else if delta_p > 0.0 {
                alpha0 + phi_p - phi0 - 180.0
            } else {
                alpha0 - phi_p + phi0
            }
        } else {
            // [33]
            alpha0
                - arg(
                    (sin_d(theta0) - sin_d(delta_p) * sin_d(delta0))
                        / (cos_d(delta_p) * cos_d(delta0)),
                    sin_d(phi_p - phi0) * cos_d(theta0) / cos_d(delta0),
                )
        };

        // "In all cases, alpha_p is reduced to the same sign as alpha0."
        let alpha_p = if alpha0 >= 0.0 {
            reduce_ra(alpha_p)
        } else {
            let reduced = reduce_ra(alpha_p);
            if reduced > 0.0 { reduced - 360.0 } else { reduced }
        };

        Some(Rotation { pole: [alpha_p, delta_p], phi_p })
    }
}

/// A latitude from its sine and the two longitude arguments, in degrees.
///
/// `arcsin` is the obvious reading of equations \[34\] and \[35\], and it is a
/// poor one near the poles: its derivative is infinite at +-1, so an argument
/// that has drifted by one ulp comes back displaced by far more. That matters
/// here rather than in the abstract, because a zenithal projection puts its
/// reference point *at* the native pole -- the pixels nearest the centre of
/// the image are the ones evaluated in the worst-conditioned region.
///
/// The specification anticipates this: "An implementation should use
/// numerically robust variants of these formulas near the poles, for example
/// computing latitudes through arccos of the modulus of the longitude
/// arguments when the sine argument approaches unity, as the reference
/// implementation does; such variants do not change the results beyond the
/// conformance tolerance of the specification."
///
/// The modulus of those two arguments is `cos(latitude)`, so taking the sine
/// and the cosine together through `atan2` is that variant without a
/// threshold to choose: it is exact at the pole and at the equator alike.
fn robust_latitude(sine: f64, a: f64, b: f64) -> f64 {
    sine.atan2(a.hypot(b)).to_degrees()
}

/// Native spherical coordinates to celestial, equation \[34\].
pub fn native_to_celestial(native: [f64; 2], rotation: Rotation) -> [f64; 2] {
    let [phi, theta] = native;
    let [alpha_p, delta_p] = rotation.pole;
    let phi_p = rotation.phi_p;

    // [36]: when the native pole coincides with a celestial pole the rotation
    // is only a change of the origin of longitude.
    if cos_d(delta_p).abs() < 1e-12 {
        return if delta_p > 0.0 {
            [reduce_ra(phi + alpha_p - phi_p + 180.0), theta]
        } else {
            [reduce_ra(alpha_p + phi_p - phi), -theta]
        };
    }

    let d_phi = phi - phi_p;
    // The two arguments of `arg` below have modulus cos(delta), which is what
    // makes the robust latitude possible.
    let a = sin_d(theta) * cos_d(delta_p) - cos_d(theta) * sin_d(delta_p) * cos_d(d_phi);
    let b = -cos_d(theta) * sin_d(d_phi);
    let alpha = alpha_p + arg(a, b);
    let delta = robust_latitude(
        sin_d(theta) * sin_d(delta_p) + cos_d(theta) * cos_d(delta_p) * cos_d(d_phi),
        a,
        b,
    );
    [reduce_ra(alpha), delta]
}

/// Celestial coordinates to native spherical, equation \[35\].
pub fn celestial_to_native(celestial: [f64; 2], rotation: Rotation) -> [f64; 2] {
    let [alpha, delta] = celestial;
    let [alpha_p, delta_p] = rotation.pole;
    let phi_p = rotation.phi_p;

    if cos_d(delta_p).abs() < 1e-12 {
        return if delta_p > 0.0 {
            [reduce(alpha - alpha_p + phi_p - 180.0), delta]
        } else {
            [reduce(alpha_p + phi_p - alpha), -delta]
        };
    }

    let d_alpha = alpha - alpha_p;
    let a = sin_d(delta) * cos_d(delta_p) - cos_d(delta) * sin_d(delta_p) * cos_d(d_alpha);
    let b = -cos_d(delta) * sin_d(d_alpha);
    let phi = phi_p + arg(a, b);
    let theta = robust_latitude(
        sin_d(delta) * sin_d(delta_p) + cos_d(delta) * cos_d(delta_p) * cos_d(d_alpha),
        a,
        b,
    );
    [reduce(phi), theta]
}

/// Native spherical coordinates to projection plane coordinates `(u, v)`.
///
/// `None` for a point outside the projection's domain -- a point behind the
/// tangent plane of a Gnomonic projection, say, which has no image at all.
pub fn project(native: [f64; 2], system: ProjectionSystem) -> Option<[f64; 2]> {
    let [phi, theta] = native;
    match system.class() {
        ProjectionClass::Zenithal => {
            let r_theta = zenithal_radius(theta, system)?;
            // [37]
            Some([r_theta * sin_d(phi), -r_theta * cos_d(phi)])
        }
        ProjectionClass::Cylindrical => match system {
            // [41]
            ProjectionSystem::PlateCarree => Some([phi, theta]),
            // [42], defined for |theta| < 90.
            ProjectionSystem::Mercator => {
                if theta.abs() >= 90.0 {
                    return None;
                }
                Some([phi, R * ((90.0 + theta) / 2.0).to_radians().tan().ln()])
            }
            _ => None,
        },
        ProjectionClass::PseudoCylindrical => {
            // [44] and [45]
            let denominator = 1.0 + cos_d(theta) * cos_d(phi / 2.0);
            if denominator <= 0.0 {
                return None;
            }
            let gamma = R * (2.0 / denominator).sqrt();
            Some([2.0 * gamma * cos_d(theta) * sin_d(phi / 2.0), gamma * sin_d(theta)])
        }
    }
}

/// Projection plane coordinates back to native spherical, the inverse of
/// [`project`].
pub fn deproject(plane: [f64; 2], system: ProjectionSystem) -> Option<[f64; 2]> {
    let [u, v] = plane;
    match system.class() {
        ProjectionClass::Zenithal => {
            // [38]
            let phi = arg(-v, u);
            // `hypot` rather than `(u*u + v*v).sqrt()`: it avoids the
            // intermediate overflow and is the more accurate of the two, which
            // matters because the inverse of a zenithal radius is sensitive
            // near the edge of its domain.
            let r_theta = u.hypot(v);
            Some([phi, zenithal_latitude(r_theta, system)?])
        }
        ProjectionClass::Cylindrical => match system {
            ProjectionSystem::PlateCarree => Some([u, v]),
            // [43]
            ProjectionSystem::Mercator => Some([u, 2.0 * (v / R).exp().atan().to_degrees() - 90.0]),
            _ => None,
        },
        ProjectionClass::PseudoCylindrical => {
            // [46]: defined only inside the boundary of the map.
            let inside = 1.0 - (u / (4.0 * R)).powi(2) - (v / (2.0 * R)).powi(2);
            if inside < 0.0 {
                return None;
            }
            let z = inside.sqrt();
            if z < core::f64::consts::FRAC_1_SQRT_2 {
                return None;
            }
            // [47]
            let phi = 2.0 * arg(2.0 * z * z - 1.0, z * u / (2.0 * R));
            let theta = (v * z / R).clamp(-1.0, 1.0).asin().to_degrees();
            Some([phi, theta])
        }
    }
}

/// `R_theta(theta)`: the radius in the projection plane, Table 19.
fn zenithal_radius(theta: f64, system: ProjectionSystem) -> Option<f64> {
    Some(match system {
        // R / tan(theta), for theta > 0.
        ProjectionSystem::Gnomonic => {
            if theta <= 0.0 {
                return None;
            }
            R / theta.to_radians().tan()
        }
        // 2R tan((90 - theta)/2), for all theta > -90.
        ProjectionSystem::Stereographic => {
            if theta <= -90.0 {
                return None;
            }
            2.0 * R * ((90.0 - theta) / 2.0).to_radians().tan()
        }
        // 2R sin((90 - theta)/2), for all theta.
        ProjectionSystem::ZenithalEqualArea => 2.0 * R * sin_d((90.0 - theta) / 2.0),
        // R cos(theta), for theta >= 0.
        ProjectionSystem::Orthographic => {
            if theta < 0.0 {
                return None;
            }
            R * cos_d(theta)
        }
        _ => return None,
    })
}

/// `theta(R_theta)`: the inverse of [`zenithal_radius`], Table 19.
fn zenithal_latitude(r_theta: f64, system: ProjectionSystem) -> Option<f64> {
    Some(match system {
        // arctan(R / R_theta)
        ProjectionSystem::Gnomonic => {
            if r_theta == 0.0 {
                90.0
            } else {
                (R / r_theta).atan().to_degrees()
            }
        }
        // 90 - 2 arctan(R_theta / 2R)
        ProjectionSystem::Stereographic => 90.0 - 2.0 * (r_theta / (2.0 * R)).atan().to_degrees(),
        // 90 - 2 arcsin(R_theta / 2R), with R_theta <= 2R
        ProjectionSystem::ZenithalEqualArea => {
            let s = r_theta / (2.0 * R);
            if s > 1.0 {
                return None;
            }
            90.0 - 2.0 * s.asin().to_degrees()
        }
        // arccos(R_theta / R), with R_theta <= R
        ProjectionSystem::Orthographic => {
            let c = r_theta / R;
            if c > 1.0 {
                return None;
            }
            c.acos().to_degrees()
        }
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `arg` takes its arguments in the opposite order to `atan2`, which is
    /// exactly the kind of thing that silently transposes a sky.
    #[test]
    fn arg_follows_the_specifications_argument_order() {
        // (cos psi, sin psi) proportional to (x, y): arg(1, 0) is 0 degrees,
        // arg(0, 1) is 90.
        assert!((arg(1.0, 0.0) - 0.0).abs() < 1e-12);
        assert!((arg(0.0, 1.0) - 90.0).abs() < 1e-12);
        assert!((arg(-1.0, 0.0) - 180.0).abs() < 1e-12, "the range is (-180, 180]");
        assert!((arg(0.0, -1.0) + 90.0).abs() < 1e-12);
    }

    /// Every projection must round-trip: project then deproject returns the
    /// point it started from. This is the property that catches a transposed
    /// sign or a swapped argument in either direction, since an error in one
    /// is not generally undone by the other.
    #[test]
    fn every_projection_round_trips() {
        // The Orthographic limb, theta = 0, is excluded deliberately. There
        // R_theta = R and the inverse is arccos(1), whose derivative is
        // infinite: the projection is genuinely singular at its own edge, so
        // a round trip through (u, v) loses most of its digits there for any
        // implementation. It is tested separately, at the accuracy the point
        // actually admits, rather than being smuggled past with a loose
        // tolerance here.
        for system in [
            ProjectionSystem::Gnomonic,
            ProjectionSystem::Stereographic,
            ProjectionSystem::ZenithalEqualArea,
            ProjectionSystem::Orthographic,
            ProjectionSystem::PlateCarree,
            ProjectionSystem::Mercator,
            ProjectionSystem::HammerAitoff,
        ] {
            for &phi in &[-170.0, -90.0, -30.0, 0.0, 45.0, 120.0, 179.0] {
                for &theta in &[-80.0, -40.0, 0.0, 15.0, 60.0, 89.0] {
                    if system == ProjectionSystem::Orthographic && theta == 0.0 {
                        continue;
                    }
                    let Some(plane) = project([phi, theta], system) else { continue };
                    let Some([phi2, theta2]) = deproject(plane, system) else {
                        panic!("{} could not deproject {plane:?}", system.name())
                    };
                    assert!(
                        (theta - theta2).abs() < 1e-9,
                        "{}: latitude {theta} came back {theta2}",
                        system.name()
                    );
                    // Longitude is meaningless at a pole, where cos(theta) = 0.
                    if theta.abs() < 89.9 {
                        assert!(
                            (reduce(phi - phi2)).abs() < 1e-9,
                            "{}: longitude {phi} came back {phi2}",
                            system.name()
                        );
                    }
                }
            }
        }
    }

    /// The Orthographic projection is singular at its limb: it maps the
    /// visible hemisphere onto a disc of radius R, and latitude 0 lands on the
    /// boundary, where the inverse is arccos of a value at 1 and loses about
    /// half its significant digits. This pins what the point actually
    /// achieves, so that a future change that makes it worse is visible.
    /// Not run under Miri, which deliberately perturbs floating point maths to
    /// catch code that relies on exact results. This test measures how much
    /// precision is lost at a singularity, so a deliberately imprecise
    /// interpreter has nothing to say about it.
    #[cfg_attr(miri, ignore)]
    #[test]
    fn the_orthographic_limb_is_singular_but_bounded() {
        let system = ProjectionSystem::Orthographic;
        let plane = project([30.0, 0.0], system).expect("the limb is in domain");
        let [_, theta] = deproject(plane, system).expect("and deprojects");
        assert!(theta.abs() < 1e-5, "the limb came back at {theta}, further out than expected");

        // One degree in from the limb the conditioning is already ordinary.
        let plane = project([30.0, 1.0], system).expect("in domain");
        let [_, theta] = deproject(plane, system).expect("deprojects");
        assert!((theta - 1.0).abs() < 1e-9, "one degree in came back {theta}");
    }

    /// The native pole maps to the origin of the projection plane for every
    /// zenithal projection, which is what "tangent at, or centered on, the
    /// native pole" means.
    #[test]
    fn zenithal_projections_put_the_native_pole_at_the_origin() {
        for system in [
            ProjectionSystem::Gnomonic,
            ProjectionSystem::Stereographic,
            ProjectionSystem::ZenithalEqualArea,
            ProjectionSystem::Orthographic,
        ] {
            let [u, v] = project([0.0, 90.0], system).expect("the pole is always in domain");
            assert!(u.abs() < 1e-9 && v.abs() < 1e-9, "{}: pole at ({u}, {v})", system.name());
        }
    }

    /// A Gnomonic projection cannot see the hemisphere behind it.
    #[test]
    fn the_gnomonic_domain_is_the_visible_hemisphere() {
        assert!(project([0.0, 45.0], ProjectionSystem::Gnomonic).is_some());
        assert!(project([0.0, 0.0], ProjectionSystem::Gnomonic).is_none());
        assert!(project([0.0, -10.0], ProjectionSystem::Gnomonic).is_none());
    }

    /// Plate Carree is "the identity on native coordinates".
    #[test]
    fn plate_carree_is_the_identity() {
        assert_eq!(project([42.0, -17.0], ProjectionSystem::PlateCarree), Some([42.0, -17.0]));
    }

    /// With the reference point at the native pole the rotation is the
    /// identity on the reference point itself: it must come back exactly.
    #[test]
    fn the_reference_point_maps_to_itself() {
        let reference = [10.684, 41.269];
        let rotation = Rotation::new(reference, [0.0, 90.0], None).expect("valid parameters");
        // theta0 = 90 means the reference point *is* the native pole, [29].
        assert_eq!(rotation.pole, reference);

        // The native pole in native coordinates is (anything, 90).
        let [alpha, delta] = native_to_celestial([0.0, 90.0], rotation);
        assert!((alpha - reference[0]).abs() < 1e-9, "alpha {alpha}");
        assert!((delta - reference[1]).abs() < 1e-9, "delta {delta}");
    }

    /// The rotation must round-trip for points away from the pole.
    #[test]
    fn the_spherical_rotation_round_trips() {
        let rotation =
            Rotation::new([83.822, -5.391], [0.0, 90.0], None).expect("valid parameters");
        for &phi in &[-150.0, -60.0, 0.0, 30.0, 170.0] {
            for &theta in &[-45.0, 0.0, 20.0, 75.0, 89.0] {
                let celestial = native_to_celestial([phi, theta], rotation);
                let [phi2, theta2] = celestial_to_native(celestial, rotation);
                assert!((theta - theta2).abs() < 1e-9, "theta {theta} -> {theta2}");
                if theta.abs() < 89.9 {
                    assert!(reduce(phi - phi2).abs() < 1e-9, "phi {phi} -> {phi2}");
                }
            }
        }
    }
}
