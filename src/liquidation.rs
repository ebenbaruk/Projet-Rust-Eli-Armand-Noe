//! Liquidation rule (section 4.5 of the specification).
//!
//! When the health factor of a position drops below 1, anyone can repay part of
//! its debt and receive collateral with a bonus in exchange. This module only
//! applies that rule to one position. Choosing which position to liquidate, and
//! when, is the job of the liquidation bots and of the stress test.

use serde::Serialize;

use crate::error::LendingError;
use crate::protocol::Protocol;
use crate::types::{BPS_SCALE, Bps, EthAmount, UsdcAmount, UserId, to_u64};

/// What one liquidation changed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Liquidation {
    /// Owner of the liquidated position.
    pub user: UserId,
    /// Debt paid back by the liquidator.
    pub repaid: UsdcAmount,
    /// Collateral handed to the liquidator, bonus included.
    pub seized: EthAmount,
}

/// Repays up to `amount` of the debt of an unhealthy position, in exchange for
/// its collateral.
///
/// The repaid amount is capped at the close factor of the parameters (50 % of
/// the debt). The liquidator receives `repaid × (1 + bonus) / price` in ETH.
///
/// Fails with `ZeroAmount`, `UnknownUser`, `PositionHealthy` when the health
/// factor is at least 1, or `NothingToLiquidate` when the position has no
/// collateral left to seize.
pub fn liquidate(
    protocol: &mut Protocol,
    user: &UserId,
    amount: UsdcAmount,
) -> Result<Liquidation, LendingError> {
    if amount.0 == 0 {
        return Err(LendingError::ZeroAmount);
    }
    let params = protocol.params;
    let price = protocol.price;
    let mut position = protocol.position(user)?;
    if !position.is_liquidatable(price, params.liquidation_threshold) {
        return Err(LendingError::PositionHealthy);
    }

    let mut repaid = amount.min(params.close_factor.apply_to(position.debt));
    let mut seized = add_bonus(repaid, params.liquidation_bonus).to_eth(price);

    // Not enough collateral left: the liquidator takes all of it, but only
    // repays what it is worth. Repaying more would make the bad debt vanish,
    // and no real liquidator would pay for collateral that does not exist.
    if seized > position.collateral {
        seized = position.collateral;
        repaid = remove_bonus(position.collateral_value(price), params.liquidation_bonus);
    }
    if repaid.0 == 0 {
        return Err(LendingError::NothingToLiquidate);
    }

    position.debt.0 -= repaid.0;
    position.collateral.0 -= seized.0;
    protocol.cash.0 = protocol.cash.0.saturating_add(repaid.0);
    protocol.positions.insert(user.clone(), position);

    Ok(Liquidation {
        user: user.clone(),
        repaid,
        seized,
    })
}

/// Returns `amount × (1 + bonus)`.
fn add_bonus(amount: UsdcAmount, bonus: Bps) -> UsdcAmount {
    UsdcAmount(amount.0.saturating_add(bonus.apply_to(amount).0))
}

/// Returns `value / (1 + bonus)`: the debt that buys `value` of collateral.
fn remove_bonus(value: UsdcAmount, bonus: Bps) -> UsdcAmount {
    let scaled = value.0 as u128 * BPS_SCALE as u128;
    UsdcAmount(to_u64(scaled / (BPS_SCALE + bonus.0) as u128))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{GWEI_PER_ETH, Params, Price};

    /// Bob has 1 ETH and owes 1 500 USDC. At 1 800 USDC per ETH, his health
    /// factor is 1 800 × 0.8 / 1 500 = 0.96, so he can be liquidated.
    fn protocol_with_unhealthy_bob() -> Protocol {
        let mut protocol = Protocol::new(Params::default(), Price::from_usdc(2_000));
        protocol
            .supply(&UserId::new("alice"), UsdcAmount::from_usdc(10_000))
            .unwrap();
        let bob = UserId::new("bob");
        protocol.deposit(&bob, EthAmount::from_eth(1)).unwrap();
        protocol.borrow(&bob, UsdcAmount::from_usdc(1_500)).unwrap();
        protocol.price = Price::from_usdc(1_800);
        protocol
    }

    #[test]
    fn repaid_amount_is_capped_at_half_the_debt() {
        let mut protocol = protocol_with_unhealthy_bob();
        let bob = UserId::new("bob");
        let cash_before = protocol.cash;

        let liquidation = liquidate(&mut protocol, &bob, UsdcAmount::from_usdc(1_500)).unwrap();

        assert_eq!(liquidation.repaid, UsdcAmount::from_usdc(750));
        let position = protocol.position(&bob).unwrap();
        assert_eq!(position.debt, UsdcAmount::from_usdc(750));
        assert_eq!(protocol.cash.0, cash_before.0 + liquidation.repaid.0);
    }

    #[test]
    fn liquidator_receives_the_bonus() {
        let mut protocol = protocol_with_unhealthy_bob();
        let bob = UserId::new("bob");

        let liquidation = liquidate(&mut protocol, &bob, UsdcAmount::from_usdc(750)).unwrap();

        // 750 USDC + 5 % = 787.5 USDC, i.e. 0.4375 ETH at 1 800 USDC.
        let expected = EthAmount(GWEI_PER_ETH * 4_375 / 10_000);
        assert_eq!(liquidation.seized, expected);
        let position = protocol.position(&bob).unwrap();
        assert_eq!(position.collateral.0, GWEI_PER_ETH - expected.0);
    }

    #[test]
    fn a_smaller_amount_is_repaid_as_asked() {
        let mut protocol = protocol_with_unhealthy_bob();
        let liquidation = liquidate(
            &mut protocol,
            &UserId::new("bob"),
            UsdcAmount::from_usdc(100),
        )
        .unwrap();
        assert_eq!(liquidation.repaid, UsdcAmount::from_usdc(100));
    }

    #[test]
    fn all_the_collateral_goes_when_it_is_not_enough() {
        let mut protocol = protocol_with_unhealthy_bob();
        let bob = UserId::new("bob");
        // At 700 USDC, 750 USDC + 5 % would need 1.125 ETH, but Bob only has 1.
        protocol.price = Price::from_usdc(700);

        let liquidation = liquidate(&mut protocol, &bob, UsdcAmount::from_usdc(750)).unwrap();

        assert_eq!(liquidation.seized, EthAmount::from_eth(1));
        // 700 USDC / 1.05 = 666.66 USDC.
        assert_eq!(liquidation.repaid, UsdcAmount(66_666));
        let position = protocol.position(&bob).unwrap();
        assert_eq!(position.collateral, EthAmount(0));
        assert_eq!(
            position.bad_debt(protocol.price),
            UsdcAmount(150_000 - 66_666)
        );
    }

    #[test]
    fn a_position_without_collateral_cannot_be_liquidated() {
        let mut protocol = protocol_with_unhealthy_bob();
        let bob = UserId::new("bob");
        protocol.price = Price::from_usdc(700);
        liquidate(&mut protocol, &bob, UsdcAmount::from_usdc(750)).unwrap();

        let result = liquidate(&mut protocol, &bob, UsdcAmount::from_usdc(750));
        assert_eq!(result, Err(LendingError::NothingToLiquidate));
    }

    #[test]
    fn a_healthy_position_is_left_untouched() {
        let mut protocol = protocol_with_unhealthy_bob();
        protocol.price = Price::from_usdc(2_000);
        let before = protocol.clone();

        let result = liquidate(
            &mut protocol,
            &UserId::new("bob"),
            UsdcAmount::from_usdc(100),
        );

        assert_eq!(result, Err(LendingError::PositionHealthy));
        assert_eq!(protocol, before);
    }

    #[test]
    fn zero_amount_is_refused() {
        let mut protocol = protocol_with_unhealthy_bob();
        let result = liquidate(&mut protocol, &UserId::new("bob"), UsdcAmount(0));
        assert_eq!(result, Err(LendingError::ZeroAmount));
    }

    #[test]
    fn unknown_user_is_refused() {
        let mut protocol = protocol_with_unhealthy_bob();
        let carol = UserId::new("carol");
        let result = liquidate(&mut protocol, &carol, UsdcAmount::from_usdc(100));
        assert_eq!(result, Err(LendingError::UnknownUser(carol)));
    }
}
