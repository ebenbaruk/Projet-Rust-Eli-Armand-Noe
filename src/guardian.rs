//! Risk guardian: tightens the borrowing rules when the price of ETH falls fast.
//!
//! The guardian only acts on new borrows, through the `borrowing_paused` and
//! `max_ltv_override` fields of `Protocol`. Existing positions are never touched.

use std::collections::VecDeque;

use serde::Serialize;

use crate::protocol::Protocol;
use crate::types::{BPS_SCALE, Bps, Price, to_u64};

/// Number of minutes over which the drop of the price is measured.
pub const WINDOW_MINUTES: usize = 60;

/// Drop from which new borrows are limited.
pub const ALERT_DROP: Bps = Bps(500);

/// Drop from which new borrows are paused.
pub const EMERGENCY_DROP: Bps = Bps(1_500);

/// LTV of new borrows in `Alert` mode.
pub const ALERT_MAX_LTV: Bps = Bps(5_000);

/// How strict the guardian is with new borrows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum RiskMode {
    /// Normal LTV.
    Normal,
    /// LTV lowered to `ALERT_MAX_LTV`.
    Alert,
    /// New borrows are paused.
    Emergency,
}

impl RiskMode {
    /// Returns the mode that matches a drop of the price over the window.
    pub fn from_drop(drop: Bps) -> RiskMode {
        if drop >= EMERGENCY_DROP {
            RiskMode::Emergency
        } else if drop >= ALERT_DROP {
            RiskMode::Alert
        } else {
            RiskMode::Normal
        }
    }
}

/// Watches the price of ETH and restricts new borrows during a crash.
#[derive(Debug, Clone)]
pub struct Guardian {
    enabled: bool,
    /// Prices of the last `WINDOW_MINUTES` minutes, the latest at the back.
    prices: VecDeque<Price>,
    price_drop: Bps,
}

impl Guardian {
    /// Creates a guardian that has not seen any price yet.
    pub fn new(enabled: bool) -> Guardian {
        Guardian {
            enabled,
            prices: VecDeque::with_capacity(WINDOW_MINUTES),
            price_drop: Bps(0),
        }
    }

    /// Returns true when the guardian restricts new borrows.
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Returns the drop between the highest price of the window and the latest one.
    pub fn price_drop(&self) -> Bps {
        self.price_drop
    }

    /// Returns the mode applied to new borrows. A disabled guardian is always `Normal`.
    pub fn mode(&self) -> RiskMode {
        if self.enabled {
            RiskMode::from_drop(self.price_drop)
        } else {
            RiskMode::Normal
        }
    }

    /// Records the current price of the protocol, then updates its borrowing rules.
    ///
    /// Called once per simulated minute. Returns the new mode.
    pub fn observe(&mut self, protocol: &mut Protocol) -> RiskMode {
        if self.prices.len() == WINDOW_MINUTES {
            self.prices.pop_front();
        }
        self.prices.push_back(protocol.price);

        let price = protocol.price.0;
        let high = self.prices.iter().map(|p| p.0).max().unwrap_or(price);
        let drop = (high - price) as u128 * BPS_SCALE as u128;
        self.price_drop = Bps(to_u64(drop.checked_div(high as u128).unwrap_or(0)));

        self.apply(protocol);
        self.mode()
    }

    /// Turns the guardian on or off and updates the borrowing rules right away.
    pub fn set_enabled(&mut self, enabled: bool, protocol: &mut Protocol) {
        self.enabled = enabled;
        self.apply(protocol);
    }

    fn apply(&self, protocol: &mut Protocol) {
        let (paused, max_ltv) = match self.mode() {
            RiskMode::Normal => (false, None),
            RiskMode::Alert => (false, Some(ALERT_MAX_LTV)),
            RiskMode::Emergency => (true, None),
        };
        protocol.borrowing_paused = paused;
        protocol.max_ltv_override = max_ltv;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::LendingError;
    use crate::types::{EthAmount, Params, UsdcAmount, UserId};

    /// Feeds the prices to the guardian, one per minute, and returns the last mode.
    fn replay(guardian: &mut Guardian, protocol: &mut Protocol, prices: &[u64]) -> RiskMode {
        let mut mode = guardian.mode();
        for &price in prices {
            protocol.price = Price::from_usdc(price);
            mode = guardian.observe(protocol);
        }
        mode
    }

    fn protocol() -> Protocol {
        Protocol::new(Params::default(), Price::from_usdc(2_000))
    }

    #[test]
    fn modes_follow_the_drop_thresholds() {
        assert_eq!(RiskMode::from_drop(Bps(499)), RiskMode::Normal);
        assert_eq!(RiskMode::from_drop(Bps(500)), RiskMode::Alert);
        assert_eq!(RiskMode::from_drop(Bps(1_499)), RiskMode::Alert);
        assert_eq!(RiskMode::from_drop(Bps(1_500)), RiskMode::Emergency);
    }

    #[test]
    fn stable_price_keeps_the_normal_rules() {
        let mut protocol = protocol();
        let mut guardian = Guardian::new(true);
        let mode = replay(&mut guardian, &mut protocol, &[2_000, 2_010, 1_990]);
        assert_eq!(mode, RiskMode::Normal);
        assert!(!protocol.borrowing_paused);
        assert_eq!(protocol.max_ltv_override, None);
    }

    #[test]
    fn drop_of_5_percent_lowers_the_ltv() {
        let mut protocol = protocol();
        let mut guardian = Guardian::new(true);
        let mode = replay(&mut guardian, &mut protocol, &[2_000, 1_900]);
        assert_eq!(guardian.price_drop(), Bps(500));
        assert_eq!(mode, RiskMode::Alert);
        assert_eq!(protocol.current_max_ltv(), ALERT_MAX_LTV);
    }

    #[test]
    fn drop_of_15_percent_pauses_borrowing() {
        let mut protocol = protocol();
        protocol
            .deposit(&UserId::new("bob"), EthAmount::from_eth(1))
            .unwrap();
        let mut guardian = Guardian::new(true);
        let mode = replay(&mut guardian, &mut protocol, &[2_000, 1_700]);
        assert_eq!(mode, RiskMode::Emergency);
        assert_eq!(
            protocol.borrow(&UserId::new("bob"), UsdcAmount::from_usdc(100)),
            Err(LendingError::BorrowingPaused)
        );
    }

    #[test]
    fn old_high_leaves_the_window() {
        let mut protocol = protocol();
        let mut guardian = Guardian::new(true);
        replay(&mut guardian, &mut protocol, &[2_000, 1_700]);
        // 59 more minutes at 1 700: the 2 000 high is 60 minutes old and dropped.
        let mode = replay(&mut guardian, &mut protocol, &[1_700; WINDOW_MINUTES - 1]);
        assert_eq!(mode, RiskMode::Normal);
        assert!(!protocol.borrowing_paused);
    }

    #[test]
    fn disabled_guardian_measures_but_does_not_restrict() {
        let mut protocol = protocol();
        let mut guardian = Guardian::new(false);
        let mode = replay(&mut guardian, &mut protocol, &[2_000, 1_000]);
        assert_eq!(guardian.price_drop(), Bps(5_000));
        assert_eq!(mode, RiskMode::Normal);
        assert!(!protocol.borrowing_paused);

        guardian.set_enabled(true, &mut protocol);
        assert_eq!(guardian.mode(), RiskMode::Emergency);
        assert!(protocol.borrowing_paused);
    }
}
