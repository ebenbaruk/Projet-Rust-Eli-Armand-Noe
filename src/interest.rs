//! Interest rate model of the pool.
//!
//! The rate follows the same two-slope model as Aave: cheap while the pool has
//! liquidity, then sharply more expensive above the optimal utilization.

use crate::types::{BPS_SCALE, Bps, Params, UsdcAmount, to_u64};

/// Returns `value x numerator / denominator`, or 0 when the denominator is 0.
fn scale(value: u64, numerator: u64, denominator: u64) -> u64 {
    if denominator == 0 {
        return 0;
    }
    to_u64(value as u128 * numerator as u128 / denominator as u128)
}

/// Returns the share of the pool that is currently lent out.
///
/// The USDC supplied by lenders is either still in the pool (`cash`) or
/// borrowed, so the utilization is `borrowed / (cash + borrowed)`.
pub fn utilization(borrowed: UsdcAmount, cash: UsdcAmount) -> Bps {
    let total = borrowed.0.saturating_add(cash.0);
    Bps(scale(borrowed.0, BPS_SCALE, total))
}

/// Returns the yearly borrow rate for a given utilization.
pub fn annual_rate(utilization: Bps, params: &Params) -> Bps {
    let optimal = params.optimal_utilization.0;
    let base = params.base_rate.0;

    if utilization.0 <= optimal {
        let low = scale(params.slope_low.0, utilization.0, optimal);
        return Bps(base + low);
    }

    // Above the optimal utilization, the full low slope applies, plus a share
    // of the high slope that grows with the excess utilization.
    let excess = utilization.0 - optimal;
    let room = BPS_SCALE.saturating_sub(optimal);
    let high = scale(params.slope_high.0, excess, room);
    Bps(base + params.slope_low.0 + high)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utilization_of_the_pool() {
        let borrowed = UsdcAmount::from_usdc(800);
        let cash = UsdcAmount::from_usdc(200);
        assert_eq!(utilization(borrowed, cash), Bps(8_000));
    }

    #[test]
    fn utilization_of_an_empty_pool_is_zero() {
        assert_eq!(utilization(UsdcAmount(0), UsdcAmount(0)), Bps(0));
        assert_eq!(utilization(UsdcAmount(0), UsdcAmount(500)), Bps(0));
    }

    #[test]
    fn rate_below_the_optimal_utilization() {
        let params = Params::default();
        assert_eq!(annual_rate(Bps(0), &params), Bps(200));
        assert_eq!(annual_rate(Bps(4_000), &params), Bps(400));
        assert_eq!(annual_rate(Bps(8_000), &params), Bps(600));
    }

    #[test]
    fn rate_above_the_optimal_utilization() {
        let params = Params::default();
        // 6 % + (90 % - 80 %) / 20 % x 60 % = 36 %
        assert_eq!(annual_rate(Bps(9_000), &params), Bps(3_600));
        assert_eq!(annual_rate(Bps(10_000), &params), Bps(6_600));
    }
}
