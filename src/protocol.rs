//! State of the lending pool and the lending operations.
//!
//! `Protocol` owns every position and the USDC held by the pool. All the rules
//! of section 4.4 of the specification are checked here.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::error::LendingError;
use crate::interest;
use crate::types::{Bps, EthAmount, Params, Position, Price, UsdcAmount, UserId};

/// The whole state of the lending pool.
///
/// It derives `Serialize` and `Deserialize` so it can be saved to a JSON file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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

    /// Returns the LTV that applies to new borrows.
    ///
    /// It is the LTV of the parameters, unless the risk guardian has lowered it.
    pub fn current_max_ltv(&self) -> Bps {
        self.max_ltv_override.unwrap_or(self.params.max_ltv)
    }

    /// Lends USDC from the pool to a user, against their collateral.
    ///
    /// Returns the updated position. Fails with `ZeroAmount`, `BorrowingPaused`,
    /// `UnknownUser`, `InsufficientCollateral` if the new debt would exceed the
    /// borrowing limit, or `InsufficientLiquidity` if the pool lacks USDC.
    pub fn borrow(&mut self, user: &UserId, amount: UsdcAmount) -> Result<Position, LendingError> {
        if amount.0 == 0 {
            return Err(LendingError::ZeroAmount);
        }
        if self.borrowing_paused {
            return Err(LendingError::BorrowingPaused);
        }

        // Work on a copy and write it back only once every check has passed,
        // so a refused borrow leaves the state untouched.
        let mut position = self.position(user)?;
        let new_debt = UsdcAmount(position.debt.0.saturating_add(amount.0));
        if new_debt > position.max_borrow(self.price, self.current_max_ltv()) {
            return Err(LendingError::InsufficientCollateral);
        }
        if amount > self.cash {
            return Err(LendingError::InsufficientLiquidity);
        }

        position.debt = new_debt;
        self.cash.0 -= amount.0;
        self.positions.insert(user.clone(), position);
        Ok(position)
    }

    /// Pays back part or all of the debt of a user.
    ///
    /// An amount larger than the debt only repays the debt. Returns the updated
    /// position, or fails with `ZeroAmount`, `UnknownUser` or `NothingToRepay`.
    pub fn repay(&mut self, user: &UserId, amount: UsdcAmount) -> Result<Position, LendingError> {
        if amount.0 == 0 {
            return Err(LendingError::ZeroAmount);
        }
        let mut position = self.position(user)?;
        if position.debt.0 == 0 {
            return Err(LendingError::NothingToRepay);
        }

        let paid = amount.min(position.debt);
        position.debt.0 -= paid.0;
        self.cash.0 = self.cash.0.saturating_add(paid.0);
        self.positions.insert(user.clone(), position);
        Ok(position)
    }

    /// Takes ETH out of the collateral of a user.
    ///
    /// The remaining collateral must still cover the debt with the LTV of the
    /// parameters. Returns the updated position, or fails with `ZeroAmount`,
    /// `UnknownUser` or `InsufficientCollateral`.
    pub fn withdraw(&mut self, user: &UserId, amount: EthAmount) -> Result<Position, LendingError> {
        if amount.0 == 0 {
            return Err(LendingError::ZeroAmount);
        }
        let mut position = self.position(user)?;
        if amount > position.collateral {
            return Err(LendingError::InsufficientCollateral);
        }

        position.collateral.0 -= amount.0;
        // The guardian only restricts new borrows, so the normal LTV is used here.
        if position.debt > position.max_borrow(self.price, self.params.max_ltv) {
            return Err(LendingError::InsufficientCollateral);
        }
        self.positions.insert(user.clone(), position);
        Ok(position)
    }

    /// Returns the total debt of all borrowers, interest included.
    pub fn total_debt(&self) -> UsdcAmount {
        let mut total = 0u64;
        for position in self.positions.values() {
            total = total.saturating_add(position.debt.0);
        }
        UsdcAmount(total)
    }

    /// Returns the debt that the collateral no longer covers, summed over every
    /// position (section 4.6). The protocol survives a crash if it is zero.
    pub fn total_bad_debt(&self) -> UsdcAmount {
        let mut total = 0u64;
        for position in self.positions.values() {
            total = total.saturating_add(position.bad_debt(self.price).0);
        }
        UsdcAmount(total)
    }

    /// Returns the share of the pool that is currently lent out.
    pub fn utilization(&self) -> Bps {
        interest::utilization(self.total_debt(), self.cash)
    }

    /// Returns the current yearly borrow rate.
    pub fn current_rate(&self) -> Bps {
        interest::annual_rate(self.utilization(), &self.params)
    }

    /// Adds one minute of interest to the debt of every borrower.
    ///
    /// All positions use the rate of the current utilization. A position
    /// without collateral earns no interest. Returns the total interest added.
    pub fn accrue_interest(&mut self) -> UsdcAmount {
        // The rate is read once, before any debt changes, so every borrower
        // pays the same rate for this minute.
        let rate = self.current_rate();
        let mut total = 0u64;
        for position in self.positions.values_mut() {
            // Debt without collateral is bad debt: nobody will repay it, so
            // letting it grow would only inflate the measured loss.
            if position.collateral.0 == 0 {
                continue;
            }
            let (interest, remainder) =
                interest::interest_for_one_minute(position.debt, position.interest_remainder, rate);
            position.debt.0 = position.debt.0.saturating_add(interest.0);
            position.interest_remainder = remainder;
            total = total.saturating_add(interest.0);
        }
        UsdcAmount(total)
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

    /// A pool with 10 000 USDC supplied by alice and 1 ETH deposited by bob.
    fn funded_protocol() -> Protocol {
        let mut protocol = new_protocol();
        protocol
            .supply(&UserId::new("alice"), UsdcAmount::from_usdc(10_000))
            .unwrap();
        protocol
            .deposit(&UserId::new("bob"), EthAmount::from_eth(1))
            .unwrap();
        protocol
    }

    #[test]
    fn borrow_up_to_the_ltv() {
        let mut protocol = funded_protocol();
        let bob = UserId::new("bob");
        // 1 ETH at 2 000 USDC with a 75 % LTV allows 1 500 USDC.
        let position = protocol.borrow(&bob, UsdcAmount::from_usdc(1_500)).unwrap();
        assert_eq!(position.debt, UsdcAmount::from_usdc(1_500));
        assert_eq!(protocol.cash, UsdcAmount::from_usdc(8_500));
    }

    #[test]
    fn borrow_above_the_ltv_is_refused() {
        let mut protocol = funded_protocol();
        let bob = UserId::new("bob");
        protocol.borrow(&bob, UsdcAmount::from_usdc(1_000)).unwrap();
        let result = protocol.borrow(&bob, UsdcAmount::from_usdc(501));
        assert_eq!(result, Err(LendingError::InsufficientCollateral));
        // The refused borrow did not change anything.
        assert_eq!(
            protocol.position(&bob).unwrap().debt,
            UsdcAmount::from_usdc(1_000)
        );
        assert_eq!(protocol.cash, UsdcAmount::from_usdc(9_000));
    }

    #[test]
    fn borrow_needs_liquidity() {
        let mut protocol = new_protocol();
        let bob = UserId::new("bob");
        protocol
            .supply(&UserId::new("alice"), UsdcAmount::from_usdc(100))
            .unwrap();
        protocol.deposit(&bob, EthAmount::from_eth(1)).unwrap();
        let result = protocol.borrow(&bob, UsdcAmount::from_usdc(200));
        assert_eq!(result, Err(LendingError::InsufficientLiquidity));
    }

    #[test]
    fn borrow_errors() {
        let mut protocol = funded_protocol();
        let bob = UserId::new("bob");
        let carol = UserId::new("carol");
        assert_eq!(
            protocol.borrow(&bob, UsdcAmount(0)),
            Err(LendingError::ZeroAmount)
        );
        assert_eq!(
            protocol.borrow(&carol, UsdcAmount::from_usdc(10)),
            Err(LendingError::UnknownUser(carol))
        );
    }

    #[test]
    fn guardian_can_pause_borrowing() {
        let mut protocol = funded_protocol();
        protocol.borrowing_paused = true;
        let result = protocol.borrow(&UserId::new("bob"), UsdcAmount::from_usdc(10));
        assert_eq!(result, Err(LendingError::BorrowingPaused));
    }

    #[test]
    fn guardian_can_lower_the_ltv() {
        let mut protocol = funded_protocol();
        let bob = UserId::new("bob");
        protocol.max_ltv_override = Some(Bps(5_000));
        assert_eq!(
            protocol.borrow(&bob, UsdcAmount::from_usdc(1_001)),
            Err(LendingError::InsufficientCollateral)
        );
        assert!(protocol.borrow(&bob, UsdcAmount::from_usdc(1_000)).is_ok());
    }

    #[test]
    fn repay_part_of_the_debt() {
        let mut protocol = funded_protocol();
        let bob = UserId::new("bob");
        protocol.borrow(&bob, UsdcAmount::from_usdc(1_000)).unwrap();
        let position = protocol.repay(&bob, UsdcAmount::from_usdc(400)).unwrap();
        assert_eq!(position.debt, UsdcAmount::from_usdc(600));
        assert_eq!(protocol.cash, UsdcAmount::from_usdc(9_400));
    }

    #[test]
    fn repay_more_than_the_debt_only_repays_the_debt() {
        let mut protocol = funded_protocol();
        let bob = UserId::new("bob");
        protocol.borrow(&bob, UsdcAmount::from_usdc(1_000)).unwrap();
        let position = protocol.repay(&bob, UsdcAmount::from_usdc(5_000)).unwrap();
        assert_eq!(position.debt, UsdcAmount(0));
        assert_eq!(protocol.cash, UsdcAmount::from_usdc(10_000));
    }

    #[test]
    fn repay_errors() {
        let mut protocol = funded_protocol();
        let bob = UserId::new("bob");
        assert_eq!(
            protocol.repay(&bob, UsdcAmount(0)),
            Err(LendingError::ZeroAmount)
        );
        assert_eq!(
            protocol.repay(&bob, UsdcAmount::from_usdc(10)),
            Err(LendingError::NothingToRepay)
        );
    }

    #[test]
    fn withdraw_without_debt() {
        let mut protocol = funded_protocol();
        let bob = UserId::new("bob");
        let position = protocol.withdraw(&bob, EthAmount::from_eth(1)).unwrap();
        assert_eq!(position.collateral, EthAmount(0));
    }

    #[test]
    fn withdraw_keeps_enough_collateral_for_the_debt() {
        let mut protocol = funded_protocol();
        let bob = UserId::new("bob");
        protocol.borrow(&bob, UsdcAmount::from_usdc(750)).unwrap();
        // Half an ETH is worth 1 000 USDC, so it still covers 750 USDC at 75 %.
        let half_eth = EthAmount::from_eth(1).0 / 2;
        assert!(protocol.withdraw(&bob, EthAmount(half_eth)).is_ok());
        // Any further withdrawal would leave the debt uncovered.
        assert_eq!(
            protocol.withdraw(&bob, EthAmount(1)),
            Err(LendingError::InsufficientCollateral)
        );
        assert_eq!(
            protocol.position(&bob).unwrap().collateral,
            EthAmount(half_eth)
        );
    }

    #[test]
    fn withdraw_errors() {
        let mut protocol = funded_protocol();
        let bob = UserId::new("bob");
        let carol = UserId::new("carol");
        assert_eq!(
            protocol.withdraw(&bob, EthAmount(0)),
            Err(LendingError::ZeroAmount)
        );
        assert_eq!(
            protocol.withdraw(&bob, EthAmount::from_eth(2)),
            Err(LendingError::InsufficientCollateral)
        );
        assert_eq!(
            protocol.withdraw(&carol, EthAmount(1)),
            Err(LendingError::UnknownUser(carol))
        );
    }

    #[test]
    fn rate_follows_the_utilization() {
        let mut protocol = funded_protocol();
        assert_eq!(protocol.utilization(), Bps(0));
        assert_eq!(protocol.current_rate(), Bps(200));

        protocol
            .borrow(&UserId::new("bob"), UsdcAmount::from_usdc(1_000))
            .unwrap();
        // 1 000 borrowed out of 10 000 supplied.
        assert_eq!(protocol.total_debt(), UsdcAmount::from_usdc(1_000));
        assert_eq!(protocol.utilization(), Bps(1_000));
        assert_eq!(protocol.current_rate(), Bps(250));
    }

    #[test]
    fn interest_increases_the_debt() {
        let mut protocol = funded_protocol();
        let bob = UserId::new("bob");
        protocol.borrow(&bob, UsdcAmount::from_usdc(1_000)).unwrap();
        let rate = protocol.current_rate();

        let mut total = UsdcAmount(0);
        for _ in 0..interest::MINUTES_PER_YEAR {
            total.0 += protocol.accrue_interest().0;
        }

        // The rate moves a little as the debt grows, so the yearly interest is
        // at least the starting rate applied to the starting debt.
        let debt = protocol.position(&bob).unwrap().debt;
        assert_eq!(debt.0, UsdcAmount::from_usdc(1_000).0 + total.0);
        assert!(total >= rate.apply_to(UsdcAmount::from_usdc(1_000)));
    }

    #[test]
    fn no_interest_without_debt() {
        let mut protocol = funded_protocol();
        assert_eq!(protocol.accrue_interest(), UsdcAmount(0));
        assert_eq!(
            protocol.position(&UserId::new("bob")).unwrap().debt,
            UsdcAmount(0)
        );
    }

    #[test]
    fn no_interest_on_a_position_without_collateral() {
        let mut protocol = funded_protocol();
        let bob = UserId::new("bob");
        protocol.borrow(&bob, UsdcAmount::from_usdc(1_000)).unwrap();
        // What a liquidation that seized every ETH leaves behind.
        protocol.positions.get_mut(&bob).unwrap().collateral = EthAmount(0);

        // One minute of interest on 1 000 USDC is less than a cent, so the
        // test runs for a whole day to make any interest visible.
        for _ in 0..24 * 60 {
            assert_eq!(protocol.accrue_interest(), UsdcAmount(0));
        }
        assert_eq!(
            protocol.position(&bob).unwrap().debt,
            UsdcAmount::from_usdc(1_000)
        );
    }

    #[test]
    fn bad_debt_appears_when_the_price_crashes() {
        let mut protocol = funded_protocol();
        protocol
            .borrow(&UserId::new("bob"), UsdcAmount::from_usdc(1_500))
            .unwrap();
        assert_eq!(protocol.total_bad_debt(), UsdcAmount(0));

        // 1 ETH now covers only 1 000 of the 1 500 USDC owed.
        protocol.price = Price::from_usdc(1_000);
        assert_eq!(protocol.total_bad_debt(), UsdcAmount::from_usdc(500));
    }

    #[test]
    fn protocol_round_trips_through_json() {
        let mut protocol = funded_protocol();
        protocol
            .borrow(&UserId::new("bob"), UsdcAmount::from_usdc(500))
            .unwrap();
        let json = serde_json::to_string(&protocol).unwrap();
        // User ids become plain JSON keys.
        assert!(json.contains("\"bob\""));
        let loaded: Protocol = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded, protocol);
    }
}
