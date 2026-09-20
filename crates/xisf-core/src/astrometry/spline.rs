//! Surface splines: the third layer of an astrometric solution.
//!
//! A distortion model adds a residual field to the projective transformation,
//! and the residual field is a normalized weighted sum of vector-valued
//! splines, one per term:
//!
//! ```text
//! T_D(p) = P_D(p) + R_D(p),   R_D(p) = sum_i w_i(p) S_i(p) / sum_i w_i(p)
//! ```
//!
//! Each term carries two scalar splines, one per output component. The weight
//! functions are what distinguish the kinds of term: a Global term has weight
//! 1 everywhere, a Local term has the compactly supported Wendland weight, and
//! a Fallback term's weight is that same function applied to how well the
//! Local terms cover the point.

use alloc::vec::Vec;

use super::BasisFunction;

/// The Wendland C2 function, which gives Local terms their weight.
///
/// ```text
/// W(t) = (1 - t)^4 (4t + 1)  if t < 1,  0 otherwise
/// ```
///
/// For a Local term `t` is the distance from the support centre in units of
/// the radius, so the weight falls to zero at the edge of the disc and the
/// normalized sum stays continuous across term boundaries. For a Fallback
/// term the argument is `s/t0` instead -- the summed Local coverage against
/// the threshold -- so the same curve runs the other way: it contributes
/// nothing where coverage is good and takes over where coverage fades.
pub fn wendland(t: f64) -> f64 {
    if t < 1.0 {
        let u = 1.0 - t;
        u * u * u * u * (4.0 * t + 1.0)
    } else {
        0.0
    }
}

/// The monomials of the polynomial part, evaluated at a normalized point.
///
/// "ordered by total degree and then by descending power of xi":
/// `1, xi, eta, xi^2, xi*eta, eta^2, xi^3, xi^2*eta, xi*eta^2, eta^3, ...`
///
/// A polynomial part of degree `m - 1` has `m(m + 1)/2` of them, which is why
/// the coefficient vector's length pins the order down.
pub fn monomials(xi: f64, eta: f64, order: i32, out: &mut Vec<f64>) {
    out.clear();
    if order < 1 {
        return;
    }
    // Degree runs to order - 1; within a degree, the power of xi descends.
    for degree in 0..order {
        for power_of_eta in 0..=degree {
            let power_of_xi = degree - power_of_eta;
            out.push(xi.powi(power_of_xi) * eta.powi(power_of_eta));
        }
    }
}

/// How many polynomial coefficients an order implies: `Q = m(m + 1)/2`, or
/// zero when the direction carries no polynomial part.
pub fn polynomial_terms(order: i32, polynomial: bool) -> usize {
    if !polynomial || order < 1 {
        return 0;
    }
    let m = order as usize;
    m * (m + 1) / 2
}

/// One scalar surface spline: a normalization, its nodes, and its
/// coefficients.
///
/// The normalization is `(x0, y0, r0)`, and a source point `p` is evaluated at
/// `q = r0 (p - p0)`. Nodes are stored already normalized, "exactly as the
/// solver used it, so that a decoder restores the model with the same
/// numerical values the encoder had" -- so they are used as given rather than
/// being normalized again.
#[derive(Clone, PartialEq, Debug)]
pub struct Spline {
    /// `(x0, y0, r0)`.
    pub normalization: [f64; 3],
    /// Normalized node coordinates.
    pub nodes: Vec<[f64; 2]>,
    /// `n` radial coefficients in node order, then `Q` polynomial ones.
    pub coefficients: Vec<f64>,
    /// The kernel's shape parameter, where it takes one.
    pub shape_parameter: Option<f64>,
}

impl Spline {
    /// Evaluate the spline at a point in the direction's source coordinates.
    ///
    /// ```text
    /// S(p) = sum_k c_k phi(|q - q_k|) + sum_j d_j M_j(q)
    /// ```
    pub fn eval(
        &self,
        point: [f64; 2],
        basis: BasisFunction,
        order: i32,
        polynomial: bool,
        scratch: &mut Vec<f64>,
    ) -> f64 {
        let [x0, y0, r0] = self.normalization;
        let xi = r0 * (point[0] - x0);
        let eta = r0 * (point[1] - y0);
        let epsilon = self.shape_parameter.unwrap_or(0.0);

        let mut value = 0.0;
        for (node, coefficient) in self.nodes.iter().zip(&self.coefficients) {
            let dx = xi - node[0];
            let dy = eta - node[1];
            value += coefficient * basis.eval((dx * dx + dy * dy).sqrt(), order, epsilon);
        }

        if polynomial {
            monomials(xi, eta, order, scratch);
            // The polynomial coefficients follow the radial ones.
            for (monomial, coefficient) in
                scratch.iter().zip(self.coefficients.iter().skip(self.nodes.len()))
            {
                value += coefficient * monomial;
            }
        }
        value
    }

    /// How many coefficients this spline should carry for a given order.
    pub fn expected_coefficients(&self, order: i32, polynomial: bool) -> usize {
        self.nodes.len() + polynomial_terms(order, polynomial)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The weight must be 1 at the centre, 0 at the edge, and C2 continuous
    /// in between -- that continuity is what keeps the normalized sum smooth
    /// where Local terms overlap.
    #[test]
    fn the_wendland_weight_runs_from_one_to_zero() {
        assert_eq!(wendland(0.0), 1.0);
        assert_eq!(wendland(1.0), 0.0);
        assert_eq!(wendland(1.5), 0.0, "outside the support it contributes nothing");
        assert!(wendland(0.5) > 0.0 && wendland(0.5) < 1.0);
        // Monotonically decreasing across the support.
        let mut previous = f64::INFINITY;
        for i in 0..=100 {
            let w = wendland(f64::from(i) / 100.0);
            assert!(w <= previous, "weight rose at t = {}", f64::from(i) / 100.0);
            previous = w;
        }
    }

    /// "ordered by total degree and then by descending power of xi".
    #[test]
    fn monomials_come_out_in_the_order_the_specification_gives() {
        let mut out = Vec::new();
        // With xi = 2 and eta = 3 every monomial has a distinct value, so the
        // sequence identifies the ordering unambiguously.
        monomials(2.0, 3.0, 4, &mut out);
        let expected = [
            1.0, // 1
            2.0, 3.0, // xi, eta
            4.0, 6.0, 9.0, // xi^2, xi*eta, eta^2
            8.0, 12.0, 18.0, 27.0, // xi^3, xi^2*eta, xi*eta^2, eta^3
        ];
        assert_eq!(out.len(), expected.len());
        // Compared with a tolerance rather than for equality. What this test
        // is about is the *order*, and these ten values are far enough apart
        // that any tolerance below one identifies the sequence uniquely --
        // whereas `powi` is not required to be exactly rounded, and under Miri
        // it deliberately is not.
        for (got, want) in out.iter().zip(expected) {
            assert!((got - want).abs() < 1e-9, "got {out:?}, wanted {expected:?}");
        }
        assert_eq!(out.len(), polynomial_terms(4, true));
    }

    #[test]
    fn the_polynomial_count_is_m_times_m_plus_one_over_two() {
        for (order, expected) in [(1, 1), (2, 3), (3, 6), (4, 10), (5, 15)] {
            assert_eq!(polynomial_terms(order, true), expected, "order {order}");
        }
        assert_eq!(polynomial_terms(4, false), 0, "no polynomial part means no terms");
    }

    /// The logarithmic kernels are defined to be zero at the origin. Floating
    /// point does not get there on its own: `0^2 * ln(0)` is `0 * -inf`, which
    /// is NaN, and one NaN would poison the whole sum.
    #[test]
    fn the_logarithmic_kernels_are_zero_at_the_origin() {
        for basis in [BasisFunction::ThinPlateSpline, BasisFunction::VariableOrder] {
            let at_origin = basis.eval(0.0, 3, 0.0);
            assert_eq!(at_origin, 0.0, "{} was {at_origin} at rho = 0", basis.name());
        }
    }

    #[test]
    fn the_kernels_match_their_definitions() {
        let e = 0.5;
        let rho = 2.0;
        assert!(
            (BasisFunction::ThinPlateSpline.eval(rho, 2, 0.0) - rho * rho * rho.ln()).abs() < 1e-12
        );
        // (rho^2)^(m-1) ln(rho^2) with m = 3: rho^4 ln(rho^2).
        let expected = rho.powi(4) * (rho * rho).ln();
        assert!((BasisFunction::VariableOrder.eval(rho, 3, 0.0) - expected).abs() < 1e-12);
        assert!(
            (BasisFunction::Gaussian.eval(rho, 0, e) - (-(e * rho).powi(2)).exp()).abs() < 1e-12
        );
        assert!(
            (BasisFunction::Multiquadric.eval(rho, 0, e) - (1.0 + (e * rho).powi(2)).sqrt()).abs()
                < 1e-12
        );
        assert!(
            (BasisFunction::InverseMultiquadric.eval(rho, 0, e)
                - 1.0 / (1.0 + (e * rho).powi(2)).sqrt())
            .abs()
                < 1e-12
        );
        assert!(
            (BasisFunction::InverseQuadratic.eval(rho, 0, e) - 1.0 / (1.0 + (e * rho).powi(2)))
                .abs()
                < 1e-12
        );
    }

    /// A spline with no radial coefficients and a first-degree polynomial is
    /// an affine function, which can be checked by hand.
    #[test]
    fn a_polynomial_only_spline_is_its_polynomial() {
        let spline = Spline {
            normalization: [10.0, 20.0, 0.5],
            nodes: Vec::new(),
            // 1, xi, eta  ->  7 + 2*xi + 3*eta
            coefficients: vec![7.0, 2.0, 3.0],
            shape_parameter: None,
        };
        let mut scratch = Vec::new();
        // q = 0.5 * (p - p0) = 0.5 * (14 - 10, 28 - 20) = (2, 4)
        let value =
            spline.eval([14.0, 28.0], BasisFunction::ThinPlateSpline, 2, true, &mut scratch);
        assert!((value - (7.0 + 2.0 * 2.0 + 3.0 * 4.0)).abs() < 1e-12, "got {value}");
    }

    /// A node sits at its own centre, where the thin plate kernel is zero, so
    /// a single-node spline with no polynomial part vanishes there.
    #[test]
    fn a_spline_vanishes_at_its_own_node_under_a_logarithmic_kernel() {
        let spline = Spline {
            normalization: [0.0, 0.0, 1.0],
            nodes: vec![[3.0, 4.0]],
            coefficients: vec![5.0],
            shape_parameter: None,
        };
        let mut scratch = Vec::new();
        let at_node =
            spline.eval([3.0, 4.0], BasisFunction::ThinPlateSpline, 2, false, &mut scratch);
        assert_eq!(at_node, 0.0, "the kernel should be zero at zero distance");
    }
}
