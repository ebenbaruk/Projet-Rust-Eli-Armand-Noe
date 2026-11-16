//! CrashLend: a simplified lending protocol, simulated in memory.
//!
//! The library holds all the business logic. The binary in `main.rs` only starts the program.

pub mod api;
pub mod bots;
pub mod error;
pub mod guardian;
pub mod interest;
pub mod liquidation;
pub mod persistence;
pub mod protocol;
pub mod replay;
pub mod report;
pub mod stress_test;
pub mod types;
