//! The error type shared by the whole library.
//!
//! Every function that can fail returns `Result<T, LendingError>`.

use std::fmt;

use crate::types::UserId;

/// Everything that can go wrong in CrashLend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LendingError {
    /// The amount of an operation is zero.
    ZeroAmount,
    /// No position exists for this user.
    UnknownUser(UserId),
    /// The borrow or withdrawal would push the debt above the borrowing limit.
    InsufficientCollateral,
    /// The pool does not hold enough USDC for this borrow.
    InsufficientLiquidity,
    /// The user tries to repay but has no debt.
    NothingToRepay,
    /// The position cannot be liquidated because its health factor is at least 1.
    PositionHealthy,
    /// The risk guardian has paused new borrows.
    BorrowingPaused,
    /// No crash scenario has this id.
    ScenarioNotFound(String),
    /// A replay is already in progress.
    ReplayAlreadyRunning,
    /// A JSON file could not be read or written.
    Storage(String),
}

impl fmt::Display for LendingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LendingError::ZeroAmount => write!(f, "amount must be greater than zero"),
            LendingError::UnknownUser(user) => write!(f, "unknown user: {}", user.0),
            LendingError::InsufficientCollateral => {
                write!(f, "not enough collateral for this operation")
            }
            LendingError::InsufficientLiquidity => write!(f, "not enough USDC in the pool"),
            LendingError::NothingToRepay => write!(f, "this position has no debt to repay"),
            LendingError::PositionHealthy => {
                write!(f, "position is healthy and cannot be liquidated")
            }
            LendingError::BorrowingPaused => write!(f, "borrowing is paused by the risk guardian"),
            LendingError::ScenarioNotFound(id) => write!(f, "scenario not found: {id}"),
            LendingError::ReplayAlreadyRunning => write!(f, "a replay is already running"),
            LendingError::Storage(message) => write!(f, "storage error: {message}"),
        }
    }
}

impl std::error::Error for LendingError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_are_readable() {
        assert_eq!(
            LendingError::ZeroAmount.to_string(),
            "amount must be greater than zero"
        );
        assert_eq!(
            LendingError::UnknownUser(UserId::new("bob")).to_string(),
            "unknown user: bob"
        );
        assert_eq!(
            LendingError::Storage("file missing".to_string()).to_string(),
            "storage error: file missing"
        );
    }
}
