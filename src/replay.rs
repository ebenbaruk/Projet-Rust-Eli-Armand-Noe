//! Replay of a crash, minute by minute (section 5.3).
//!
//! The replay is a tokio task: it spends most of its time waiting for the next
//! step, so it does not need a thread of its own. At each step it sets the
//! price, adds one minute of interest and informs the risk guardian. The
//! liquidation bots run next to it as other tasks and share the same state.

use std::time::Duration;

use serde::Serialize;
use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::api::AppState;
use crate::bots::{self, Bot, BotReport, DEFAULT_BOTS};
use crate::error::LendingError;
use crate::liquidation::Liquidation;
use crate::report::ReplayReport;
use crate::types::Price;

/// Real time between two simulated minutes. A day of 1 440 minutes takes
/// about 15 seconds.
pub const DEFAULT_STEP_DELAY: Duration = Duration::from_millis(10);

/// How a replay runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplayConfig {
    /// Real time between two simulated minutes.
    pub step_delay: Duration,
    /// Bots that compete to liquidate the unhealthy positions.
    pub bots: Vec<Bot>,
}

impl Default for ReplayConfig {
    /// Returns the setup of the demo: 10 ms per minute and three bots.
    fn default() -> ReplayConfig {
        ReplayConfig {
            step_delay: DEFAULT_STEP_DELAY,
            bots: DEFAULT_BOTS.to_vec(),
        }
    }
}

/// The current minute of the replay, sent to the bots.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Clock {
    /// The replay is at this minute.
    Running {
        /// Index of the current price in the scenario.
        minute: usize,
    },
    /// The replay is over: the bots must stop.
    Finished,
}

/// One liquidation made by a bot during a replay.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LiquidationRecord {
    /// Minute of the scenario when the liquidation happened.
    pub minute: usize,
    /// Name of the bot that made it.
    pub bot: &'static str,
    /// What the liquidation changed.
    #[serde(flatten)]
    pub liquidation: Liquidation,
}

/// What the HTTP server needs to know about the replays.
#[derive(Debug, Default)]
pub struct ReplayState {
    /// True while a replay is in progress. Only one replay runs at a time.
    pub running: bool,
    /// Liquidations of the current or last replay.
    pub liquidations: Vec<LiquidationRecord>,
    /// Report of the last finished replay.
    pub report: Option<ReplayReport>,
}

/// Starts the replay of `prices` in the background.
///
/// Returns `ReplayAlreadyRunning` if another replay is in progress. Otherwise,
/// returns the task, which ends with the report of the replay. The report is
/// also kept in the shared state.
pub fn start(
    state: &AppState,
    scenario: &str,
    prices: Vec<Price>,
    config: ReplayConfig,
) -> Result<JoinHandle<ReplayReport>, LendingError> {
    {
        let mut replay = state.lock_replay();
        if replay.running {
            return Err(LendingError::ReplayAlreadyRunning);
        }
        replay.running = true;
        replay.liquidations.clear();
    }
    let task = tokio::spawn(run(state.clone(), scenario.to_string(), prices, config));
    Ok(task)
}

/// Replays the prices one by one while the bots liquidate, then builds the report.
async fn run(
    state: AppState,
    scenario: String,
    prices: Vec<Price>,
    config: ReplayConfig,
) -> ReplayReport {
    let (clock, clock_receiver) = watch::channel(Clock::Running { minute: 0 });
    let bot_tasks: Vec<JoinHandle<BotReport>> = config
        .bots
        .iter()
        .map(|&bot| tokio::spawn(bots::run(bot, state.clone(), clock_receiver.clone())))
        .collect();

    for (minute, &price) in prices.iter().enumerate() {
        step(&state, price);
        clock.send_replace(Clock::Running { minute });
        // No lock is held here: the bots and the HTTP handlers can work
        // while the replay waits.
        tokio::time::sleep(config.step_delay).await;
    }

    clock.send_replace(Clock::Finished);
    let mut bot_reports = Vec::new();
    for task in bot_tasks {
        match task.await {
            Ok(report) => bot_reports.push(report),
            Err(error) => eprintln!("a liquidation bot stopped unexpectedly: {error}"),
        }
    }

    let report = ReplayReport::new(&scenario, &prices, &state.lock(), bot_reports);
    let mut replay = state.lock_replay();
    replay.report = Some(report.clone());
    replay.running = false;
    report
}

/// Moves the protocol one minute forward, at the given price.
fn step(state: &AppState, price: Price) {
    let mut protocol = state.lock();
    protocol.price = price;
    protocol.accrue_interest();
    state.lock_guardian().observe(&mut protocol);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::Protocol;
    use crate::types::{EthAmount, Params, UsdcAmount, UserId};

    /// A pool where Bob owes 1 500 USDC against 1 ETH, at 2 000 USDC per ETH.
    fn state_with_bob() -> AppState {
        let mut protocol = Protocol::new(Params::default(), Price::from_usdc(2_000));
        protocol
            .supply(&UserId::new("alice"), UsdcAmount::from_usdc(10_000))
            .unwrap();
        let bob = UserId::new("bob");
        protocol.deposit(&bob, EthAmount::from_eth(1)).unwrap();
        protocol.borrow(&bob, UsdcAmount::from_usdc(1_500)).unwrap();
        AppState::new(protocol)
    }

    /// The price falls by `step` USDC per minute, from 2 000 USDC.
    fn falling_prices(minutes: u64, step: u64) -> Vec<Price> {
        (0..minutes)
            .map(|minute| Price::from_usdc(2_000 - minute * step))
            .collect()
    }

    /// A fast replay, without bots.
    fn config_without_bots() -> ReplayConfig {
        ReplayConfig {
            step_delay: Duration::from_millis(1),
            bots: Vec::new(),
        }
    }

    #[tokio::test]
    async fn every_step_moves_the_protocol_forward() {
        let state = state_with_bob();
        // -25 % in 11 minutes.
        let prices = falling_prices(11, 50);

        let report = start(&state, "test", prices, config_without_bots())
            .unwrap()
            .await
            .unwrap();

        assert_eq!(report.minutes, 11);
        let protocol = state.lock();
        assert_eq!(protocol.price, Price::from_usdc(1_500));
        // The guardian saw the fall and paused new borrows.
        assert!(protocol.borrowing_paused);
    }

    #[tokio::test]
    async fn without_bots_the_crash_leaves_bad_debt() {
        let state = state_with_bob();
        // Down to 1 000 USDC: 1 ETH no longer covers the 1 500 USDC owed.
        let prices = falling_prices(11, 100);

        let report = start(&state, "test", prices, config_without_bots())
            .unwrap()
            .await
            .unwrap();

        assert_eq!(report.liquidations, 0);
        assert!(report.bad_debt >= UsdcAmount::from_usdc(500));
    }

    #[tokio::test]
    async fn bots_liquidate_during_the_crash() {
        let state = state_with_bob();
        let config = ReplayConfig {
            step_delay: Duration::from_millis(5),
            bots: vec![Bot {
                name: "test",
                reaction: Duration::from_millis(1),
            }],
        };
        // The price stays at 1 800 USDC, where Bob can be liquidated, long
        // enough for the bot to react.
        let prices = vec![Price::from_usdc(1_800); 20];

        let report = start(&state, "test", prices, config)
            .unwrap()
            .await
            .unwrap();

        assert!(report.liquidations >= 1);
        assert_eq!(report.bad_debt, UsdcAmount(0));
        let replay = state.lock_replay();
        assert!(!replay.running);
        assert_eq!(replay.liquidations.len() as u64, report.liquidations);
        assert_eq!(replay.report.as_ref(), Some(&report));
    }

    #[tokio::test]
    async fn only_one_replay_runs_at_a_time() {
        let state = state_with_bob();
        let first = start(
            &state,
            "first",
            falling_prices(5, 10),
            config_without_bots(),
        )
        .unwrap();

        let second = start(
            &state,
            "second",
            falling_prices(5, 10),
            config_without_bots(),
        );
        assert!(matches!(second, Err(LendingError::ReplayAlreadyRunning)));

        // Once the first replay is over, a new one can start.
        first.await.unwrap();
        assert!(
            start(
                &state,
                "third",
                falling_prices(5, 10),
                config_without_bots()
            )
            .is_ok()
        );
    }
}
