//! Domain types: amounts of ETH and USDC, prices, percentages and user ids.
//!
//! Money is never stored as a float. Every value is an integer in a fixed unit,
//! and intermediate products are computed in u128 so they cannot overflow.

use serde::{Deserialize, Serialize};

/// Number of gwei in one ETH.
pub const GWEI_PER_ETH: u64 = 1_000_000_000;

/// Number of cents in one USDC.
pub const CENTS_PER_USDC: u64 = 100;

/// Number of basis points in 100 %.
pub const BPS_SCALE: u64 = 10_000;

/// Converts a u128 result back to u64. Values too large for a u64 are capped at `u64::MAX`.
pub(crate) fn to_u64(value: u128) -> u64 {
    u64::try_from(value).unwrap_or(u64::MAX)
}

/// A quantity of ETH, in gwei (1 ETH = 10^9 gwei).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Serialize, Deserialize)]
pub struct EthAmount(pub u64);

impl EthAmount {
    /// Builds an amount from a whole number of ETH.
    pub fn from_eth(eth: u64) -> EthAmount {
        EthAmount(eth.saturating_mul(GWEI_PER_ETH))
    }

    /// Returns the value of this quantity of ETH in USDC at the given price.
    pub fn value_at(self, price: Price) -> UsdcAmount {
        let value = self.0 as u128 * price.0 as u128 / GWEI_PER_ETH as u128;
        UsdcAmount(to_u64(value))
    }
}

/// An amount of USDC, in cents (1 USDC = 100 cents).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Serialize, Deserialize)]
pub struct UsdcAmount(pub u64);

impl UsdcAmount {
    /// Builds an amount from a whole number of USDC.
    pub fn from_usdc(usdc: u64) -> UsdcAmount {
        UsdcAmount(usdc.saturating_mul(CENTS_PER_USDC))
    }

    /// Returns how much ETH this amount buys at the given price.
    ///
    /// A zero price makes ETH free, so the result is capped at `u64::MAX` gwei.
    pub fn to_eth(self, price: Price) -> EthAmount {
        if price.0 == 0 {
            return EthAmount(u64::MAX);
        }
        let eth = self.0 as u128 * GWEI_PER_ETH as u128 / price.0 as u128;
        EthAmount(to_u64(eth))
    }
}

/// The price of 1 ETH, in USDC cents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Serialize, Deserialize)]
pub struct Price(pub u64);

impl Price {
    /// Builds a price from a whole number of USDC per ETH.
    pub fn from_usdc(usdc: u64) -> Price {
        Price(usdc.saturating_mul(CENTS_PER_USDC))
    }
}

/// A percentage, in basis points (1 % = 100 bps, 100 % = 10 000 bps).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Serialize, Deserialize)]
pub struct Bps(pub u64);

impl Bps {
    /// Returns this percentage of a USDC amount, rounded down.
    pub fn apply_to(self, amount: UsdcAmount) -> UsdcAmount {
        let part = amount.0 as u128 * self.0 as u128 / BPS_SCALE as u128;
        UsdcAmount(to_u64(part))
    }
}

/// Identifier of a user, such as "alice".
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct UserId(pub String);

impl UserId {
    /// Builds a user id from a name.
    pub fn new(name: &str) -> UserId {
        UserId(name.to_string())
    }
}

/// Risk and interest parameters of the protocol.
///
/// They are grouped in one struct so the stress test can try other values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Params {
    /// Maximum share of the collateral value that can be borrowed.
    pub max_ltv: Bps,
    /// Share of the collateral value above which a debt can be liquidated.
    pub liquidation_threshold: Bps,
    /// Maximum share of a debt that can be repaid in a single liquidation.
    pub close_factor: Bps,
    /// Extra collateral given to the liquidator, on top of the repaid amount.
    pub liquidation_bonus: Bps,
    /// Yearly interest rate when the pool is not used at all.
    pub base_rate: Bps,
    /// Utilization above which the interest rate rises sharply.
    pub optimal_utilization: Bps,
    /// Rate added between 0 % and the optimal utilization.
    pub slope_low: Bps,
    /// Rate added between the optimal utilization and 100 %.
    pub slope_high: Bps,
}

impl Default for Params {
    /// Returns the values of the specification (section 4.2).
    fn default() -> Params {
        Params {
            max_ltv: Bps(7_500),
            liquidation_threshold: Bps(8_000),
            close_factor: Bps(5_000),
            liquidation_bonus: Bps(500),
            base_rate: Bps(200),
            optimal_utilization: Bps(8_000),
            slope_low: Bps(400),
            slope_high: Bps(6_000),
        }
    }
}

/// The collateral and the debt of one borrower.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Position {
    /// ETH deposited as collateral.
    pub collateral: EthAmount,
    /// USDC owed, interest included.
    pub debt: UsdcAmount,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eth_value_at_price() {
        let two_eth = EthAmount::from_eth(2);
        let price = Price::from_usdc(2_000);
        assert_eq!(two_eth.value_at(price), UsdcAmount::from_usdc(4_000));
    }

    #[test]
    fn small_eth_value_is_rounded_down() {
        // 1 gwei is worth far less than a cent.
        assert_eq!(
            EthAmount(1).value_at(Price::from_usdc(2_000)),
            UsdcAmount(0)
        );
    }

    #[test]
    fn large_values_do_not_overflow() {
        let value = EthAmount(u64::MAX).value_at(Price(u64::MAX));
        assert_eq!(value, UsdcAmount(u64::MAX));
    }

    #[test]
    fn usdc_to_eth() {
        let eth = UsdcAmount::from_usdc(1_000).to_eth(Price::from_usdc(2_000));
        assert_eq!(eth, EthAmount(GWEI_PER_ETH / 2));
    }

    #[test]
    fn usdc_to_eth_with_zero_price() {
        assert_eq!(UsdcAmount(100).to_eth(Price(0)), EthAmount(u64::MAX));
    }

    #[test]
    fn bps_apply_to() {
        let amount = UsdcAmount::from_usdc(1_000);
        assert_eq!(Bps(7_500).apply_to(amount), UsdcAmount::from_usdc(750));
        assert_eq!(Bps(BPS_SCALE).apply_to(amount), amount);
        assert_eq!(Bps(0).apply_to(amount), UsdcAmount(0));
    }

    #[test]
    fn newtypes_are_plain_numbers_in_json() {
        let json = serde_json::to_string(&UsdcAmount(150_000)).unwrap();
        assert_eq!(json, "150000");
        let user: UserId = serde_json::from_str("\"alice\"").unwrap();
        assert_eq!(user, UserId::new("alice"));
    }

    #[test]
    fn default_params_match_the_specification() {
        let params = Params::default();
        assert_eq!(params.max_ltv, Bps(7_500));
        assert_eq!(params.liquidation_threshold, Bps(8_000));
        // Borrowing must always stop before the liquidation threshold.
        assert!(params.max_ltv < params.liquidation_threshold);
    }

    #[test]
    fn new_position_is_empty() {
        let position = Position::default();
        assert_eq!(position.collateral, EthAmount(0));
        assert_eq!(position.debt, UsdcAmount(0));
    }
}
