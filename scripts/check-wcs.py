#!/usr/bin/env python3
"""Compare our projection and spherical rotation against wcslib.

The specification defers to the WCS formulation for these steps -- "In case of
discrepancy, the WCS formulation is the normative reference" -- and wcslib is
that paper's own reference implementation. So this is a check against the
authority rather than against a third party.

Needs astropy:  pip install astropy
Run as:         cargo run --quiet --example wcs-dump -p xisf-core | scripts/check-wcs.py
"""
import sys
import numpy as np
from astropy.wcs import WCS
from astropy.coordinates import SkyCoord
import astropy.units as u

# The XISF projection identifiers and the WCS three-letter codes they name.
CODE = {
    "Gnomonic": "TAN",
    "Stereographic": "STG",
    "ZenithalEqualArea": "ZEA",
    "Orthographic": "SIN",
    "PlateCarree": "CAR",
    "Mercator": "MER",
    "HammerAitoff": "AIT",
}

# These must match crates/xisf-core/examples/wcs-dump.rs.
CRVAL = (10.6847, 41.2687)
REF = (512.0, 512.0)
CD = [[-0.0005, 0.00002], [0.00003, 0.0005]]
SCALE = 0.0005  # degrees per pixel, for reporting the error in pixels

# "two conforming implementations ... shall agree to within 10^-6 pixels".
TOLERANCE_PIXELS = 1e-6


def wcs_for(code):
    w = WCS(naxis=2)
    w.wcs.ctype = [f"RA---{code}", f"DEC--{code}"]
    w.wcs.crval = list(CRVAL)
    # XISF: "the pixel with column index i and row index j covers the region
    # [i, i+1] x [j, j+1], so its center is at (i + 0.5, j + 0.5)". FITS is
    # 1-based with pixel centres on integers, so a XISF coordinate r is at
    # 1-based pixel r + 0.5.
    w.wcs.crpix = [REF[0] + 0.5, REF[1] + 0.5]
    w.wcs.cd = np.array(CD)
    return w


def main():
    rows = {}
    for line in sys.stdin:
        if line.startswith("#") or not line.strip():
            continue
        system, x, y, ra, dec = line.split()
        rows.setdefault(system, []).append((float(x), float(y), float(ra), float(dec)))

    if not rows:
        print("no input: pipe `cargo run --example wcs-dump -p xisf-core` in", file=sys.stderr)
        return 2

    worst_overall = 0.0
    failed = False
    print(f"{'projection':<20}{'WCS':<5}{'points':>7}   worst separation")
    for system, points in sorted(rows.items()):
        w = wcs_for(CODE[system])
        xs = np.array([p[0] for p in points]) - 0.5   # to 0-based pixel centres
        ys = np.array([p[1] for p in points]) - 0.5
        ra_ref, dec_ref = w.wcs_pix2world(xs, ys, 0)

        ours = SkyCoord(ra=[p[2] for p in points] * u.deg, dec=[p[3] for p in points] * u.deg)
        theirs = SkyCoord(ra=ra_ref * u.deg, dec=dec_ref * u.deg)
        # Vincenty, via SkyCoord.separation. Not the arccos of a dot product:
        # near zero separation that loses about half its digits and bottoms out
        # around 3 milliarcseconds, which looks exactly like a real error.
        separation = ours.separation(theirs).to(u.microarcsecond).value

        worst = float(np.nanmax(separation))
        worst_overall = max(worst_overall, worst)
        pixels = worst / 1e6 / 3600 / SCALE
        status = "" if pixels < TOLERANCE_PIXELS else "  <== OVER TOLERANCE"
        if pixels >= TOLERANCE_PIXELS:
            failed = True
        print(f"{system:<20}{CODE[system]:<5}{len(points):>7}   {worst:9.4f} uas"
              f" = {pixels:.2e} px{status}")

    pixels = worst_overall / 1e6 / 3600 / SCALE
    print(f"\nworst overall: {worst_overall:.4f} uas = {pixels:.3e} pixels"
          f" (tolerance {TOLERANCE_PIXELS:.0e})")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
