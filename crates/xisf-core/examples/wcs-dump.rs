//! Dump image-to-celestial evaluations, for `scripts/check-wcs.py` to compare
//! against wcslib.
//!
//! This exists because the specification defers to the WCS formulation for the
//! deprojection, projection and spherical rotation steps -- "In case of
//! discrepancy, the WCS formulation is the normative reference" -- and wcslib
//! is that paper's reference implementation, by its own authors. Checking
//! against it is therefore checking against the authority, not against a third
//! party who happens to agree.
//!
//! It is worth having because every other test of these formulas is
//! self-consistency: round trips, and inverses agreeing with their forwards.
//! None of those can catch a formula that is wrong the same way in both
//! directions. This can.
use xisf_core::astrometry::*;

fn main() {
    // A plausible wide-field solution: reference point on M31, half a degree
    // per pixel is far too coarse for a real image but exercises the sphere
    // rather than a patch small enough for any formula to look right on.
    let reference_celestial = [10.6847, 41.2687];
    let reference_image = [512.0, 512.0];
    let cd = [[-0.0005, 0.00002], [0.00003, 0.0005]];

    println!("# system x y ra dec");
    for name in [
        "Gnomonic",
        "Stereographic",
        "ZenithalEqualArea",
        "Orthographic",
        "PlateCarree",
        "Mercator",
        "HammerAitoff",
    ] {
        let system = ProjectionSystem::parse(name).unwrap();
        let rotation =
            Rotation::new(reference_celestial, system.default_reference_native(), None).unwrap();

        for &x in &[0.0, 200.0, 512.0, 900.0, 1023.0] {
            for &y in &[0.0, 300.0, 512.0, 800.0, 1023.0] {
                let dx = x - reference_image[0];
                let dy = y - reference_image[1];
                let plane = [cd[0][0] * dx + cd[0][1] * dy, cd[1][0] * dx + cd[1][1] * dy];
                let Some(native) = deproject(plane, system) else { continue };
                let [ra, dec] = native_to_celestial(native, rotation);
                println!("{name} {x} {y} {ra:.12} {dec:.12}");
            }
        }
    }
}
