//! State of the lending pool and the lending operations.
//!
//! `Protocol` owns every position and the USDC held by the pool. All the rules
//! of section 4.4 of the specification are checked here.

use std::collections::BTreeMap;

use crate::error::LendingError;
use crate::types::{Bps, EthAmount, Params, Position, Price, UsdcAmount, UserId};

/// The whole state of the lending pool.
#[derive(Debug, Clone)]
pub struct Protocol {
    /// Risk and interest parameters.
    pub params: Params,
    /// Current price of 1 ETH.
    pub price: Price,
    /// USDC currently held by the pool and available for new borrows.
    pub cash: UsdcAmount,
    /// Total USDC supplied by each lender.
    pub suppliers: BTreeMap<UserId, UsdcAmount>,
    /// Position of each borrower.
    pub positions: BTreeMap<UserId, Position>,
    /// Set by the risk guardian to refuse every new borrow.
    pub borrowing_paused: bool,
    /// Set by the risk guardian to lower the LTV of new borrows.
    pub max_ltv_override: Option<Bps>,
}

impl Protocol {
    /// Creates an empty pool with the given parameters and ETH price.
    pub fn new(params: Params, price: Price) -> Protocol {
        Protocol {
            params,
            price,
            cash: UsdcAmount(0),
            suppliers: BTreeMap::new(),
            positions: BTreeMap::new(),
            borrowing_paused: false,
            max_ltv_override: None,
        }
    }

    /// Adds USDC to the pool on behalf of a lender.
    ///
    /// Returns `ZeroAmount` if the amount is zero.
    pub fn supply(&mut self, user: &UserId, amount: UsdcAmount) -> Result<(), LendingError> {
        if amount.0 == 0 {
            return Err(LendingError::ZeroAmount);
        }
        let supplied = self.suppliers.entry(user.clone()).or_default();
        supplied.0 = supplied.0.saturating_add(amount.0);
        self.cash.0 = self.cash.0.saturating_add(amount.0);
        Ok(())
    }

    /// Adds ETH to the collateral of a user, creating the position if needed.
    ///
    /// Returns the updated position, or `ZeroAmount` if the amount is zero.
    pub fn deposit(&mut self, user: &UserId, amount: EthAmount) -> Result<Position, LendingError> {
        if amount.0 == 0 {
            return Err(LendingError::ZeroAmount);
        }
        let position = self.positions.entry(user.clone()).or_default();
        position.collateral.0 = position.collateral.0.saturating_add(amount.0);
        Ok(*position)
    }

    /// Returns the position of a user, or `UnknownUser` if there is none.
    pub fn position(&self, user: &UserId) -> Result<Position, LendingError> {
        match self.positions.get(user) {
            Some(position) => Ok(*position),
            None => Err(LendingError::UnknownUser(user.clone())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn new_protocol() -> Protocol {
        Protocol::new(Params::default(), Price::from_usdc(2_000))
    }

    #[test]
    fn supply_adds_cash_to_the_pool() {
        let mut protocol = new_protocol();
        let alice = UserId::new("alice");
        protocol
            .supply(&alice, UsdcAmount::from_usdc(1_000))
            .unwrap();
        protocol.supply(&alice, UsdcAmount::from_usdc(500)).unwrap();
        assert_eq!(protocol.cash, UsdcAmount::from_usdc(1_500));
        assert_eq!(protocol.suppliers[&alice], UsdcAmount::from_usdc(1_500));
    }

    #[test]
    fn supply_of_zero_is_refused() {
        let mut protocol = new_protocol();
        let result = protocol.supply(&UserId::new("alice"), UsdcAmount(0));
        assert_eq!(result, Err(LendingError::ZeroAmount));
    }

    #[test]
    fn deposit_creates_the_position() {
        let mut protocol = new_protocol();
        let bob = UserId::new("bob");
        let position = protocol.deposit(&bob, EthAmount::from_eth(2)).unwrap();
        assert_eq!(position.collateral, EthAmount::from_eth(2));
        assert_eq!(protocol.position(&bob), Ok(position));
    }

    #[test]
    fn deposit_of_zero_is_refused() {
        let mut protocol = new_protocol();
        let result = protocol.deposit(&UserId::new("bob"), EthAmount(0));
        assert_eq!(result, Err(LendingError::ZeroAmount));
    }

    #[test]
    fn unknown_user_has_no_position() {
        let protocol = new_protocol();
        let carol = UserId::new("carol");
        assert_eq!(
            protocol.position(&carol),
            Err(LendingError::UnknownUser(carol))
        );
    }
}
