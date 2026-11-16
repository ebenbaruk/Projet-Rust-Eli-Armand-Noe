//! Liquidation bots that run next to the replay (section 5.3).
//!
//! Each bot is a tokio task. It looks for unhealthy positions, waits for its
//! reaction time, then tries to liquidate them. During that wait, a faster bot
//! may take the same positions: the slow bot then misses them, as on a real
//! blockchain where the fastest transaction wins.

use std::time::Duration;

use serde::Serialize;
use tokio::sync::watch;

use crate::api::AppState;
use crate::error::LendingError;
use crate::liquidation::{self, Liquidation};
use crate::replay::{Clock, LiquidationRecord};
use crate::types::{UsdcAmount, UserId};

/// A liquidation bot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bot {
    /// Name shown in the liquidation history and in the report.
    pub name: &'static str,
    /// Time between seeing an unhealthy position and liquidating it.
    pub reaction: Duration,
}

/// The three bots of a replay, from the fastest to the slowest.
///
/// A replay step lasts 10 ms, so they react after 2, 5 and 15 simulated minutes.
pub const DEFAULT_BOTS: [Bot; 3] = [
    Bot {
        name: "fast",
        reaction: Duration::from_millis(20),
    },
    Bot {
        name: "average",
        reaction: Duration::from_millis(50),
    },
    Bot {
        name: "slow",
        reaction: Duration::from_millis(150),
    },
];

/// What one bot did during a replay.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BotReport {
    /// Name of the bot.
    pub name: &'static str,
    /// Liquidations won by this bot.
    pub liquidations: u64,
    /// Liquidations tried too late: another bot was faster, or the position
    /// became healthy again.
    pub missed: u64,
    /// Debt repaid by this bot.
    pub repaid: UsdcAmount,
}

impl BotReport {
    fn new(name: &'static str) -> BotReport {
        BotReport {
            name,
            liquidations: 0,
            missed: 0,
            repaid: UsdcAmount(0),
        }
    }

    fn record(&mut self, liquidation: &Liquidation) {
        self.liquidations += 1;
        self.repaid.0 = self.repaid.0.saturating_add(liquidation.repaid.0);
    }
}

/// Runs a bot until the replay stops, then returns what it did.
pub async fn run(bot: Bot, state: AppState, clock: watch::Receiver<Clock>) -> BotReport {
    let mut report = BotReport::new(bot.name);
    loop {
        let targets = find_targets(&state);
        tokio::time::sleep(bot.reaction).await;

        // Copy the minute right away: the value of a watch channel must not
        // stay borrowed while the bot works.
        let minute = match *clock.borrow() {
            Clock::Running { minute } => minute,
            Clock::Finished => break,
        };
        for user in &targets {
            try_liquidate(bot, &state, user, minute, &mut report);
        }
    }
    report
}

/// Returns the positions that can be liquidated right now.
///
/// Positions without collateral are skipped: there is nothing left to seize.
fn find_targets(state: &AppState) -> Vec<UserId> {
    let protocol = state.lock();
    let price = protocol.price;
    let threshold = protocol.params.liquidation_threshold;
    protocol
        .positions
        .iter()
        .filter(|(_, position)| {
            position.collateral.0 > 0 && position.is_liquidatable(price, threshold)
        })
        .map(|(user, _)| user.clone())
        .collect()
}

/// Liquidates one position, if it is still possible, and records the result.
fn try_liquidate(bot: Bot, state: &AppState, user: &UserId, minute: usize, report: &mut BotReport) {
    let result = {
        let mut protocol = state.lock();
        // Ask for the whole debt: the liquidation rule caps it at the close factor.
        protocol
            .position(user)
            .and_then(|position| liquidation::liquidate(&mut protocol, user, position.debt))
    };

    match result {
        Ok(liquidation) => {
            report.record(&liquidation);
            state.lock_replay().liquidations.push(LiquidationRecord {
                minute,
                bot: bot.name,
                liquidation,
            });
        }
        // Another bot was faster, the price moved back up, or the debt was
        // repaid in the meantime: the opportunity is gone.
        Err(
            LendingError::PositionHealthy
            | LendingError::NothingToLiquidate
            | LendingError::ZeroAmount,
        ) => report.missed += 1,
        Err(error) => eprintln!("bot {}: cannot liquidate {}: {error}", bot.name, user.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::Protocol;
    use crate::types::{EthAmount, Params, Price};

    const TEST_BOT: Bot = Bot {
        name: "test",
        reaction: Duration::from_millis(1),
    };

    /// Bob has 1 ETH and owes 1 500 USDC. At 1 800 USDC per ETH, his health
    /// factor is 0.96, so he can be liquidated.
    fn state_with_unhealthy_bob() -> AppState {
        let mut protocol = Protocol::new(Params::default(), Price::from_usdc(2_000));
        protocol
            .supply(&UserId::new("alice"), UsdcAmount::from_usdc(10_000))
            .unwrap();
        let bob = UserId::new("bob");
        protocol.deposit(&bob, EthAmount::from_eth(1)).unwrap();
        protocol.borrow(&bob, UsdcAmount::from_usdc(1_500)).unwrap();
        protocol.price = Price::from_usdc(1_800);
        AppState::new(protocol)
    }

    #[test]
    fn unhealthy_positions_are_targets() {
        let state = state_with_unhealthy_bob();
        assert_eq!(find_targets(&state), vec![UserId::new("bob")]);

        state.lock().price = Price::from_usdc(2_000);
        assert!(find_targets(&state).is_empty());
    }

    #[test]
    fn a_liquidation_is_recorded_in_the_history() {
        let state = state_with_unhealthy_bob();
        let mut report = BotReport::new(TEST_BOT.name);

        try_liquidate(TEST_BOT, &state, &UserId::new("bob"), 7, &mut report);

        assert_eq!(report.liquidations, 1);
        assert_eq!(report.repaid, UsdcAmount::from_usdc(750));
        let history = &state.lock_replay().liquidations;
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].minute, 7);
        assert_eq!(history[0].bot, "test");
    }

    #[test]
    fn a_position_that_became_healthy_is_missed() {
        let state = state_with_unhealthy_bob();
        let mut report = BotReport::new(TEST_BOT.name);
        // The price moves back up while the bot reacts.
        state.lock().price = Price::from_usdc(2_000);

        try_liquidate(TEST_BOT, &state, &UserId::new("bob"), 0, &mut report);

        assert_eq!(report.liquidations, 0);
        assert_eq!(report.missed, 1);
        assert!(state.lock_replay().liquidations.is_empty());
    }

    #[tokio::test]
    async fn bot_stops_when_the_replay_is_finished() {
        let state = state_with_unhealthy_bob();
        let (clock, receiver) = watch::channel(Clock::Running { minute: 0 });
        let task = tokio::spawn(run(TEST_BOT, state.clone(), receiver));

        tokio::time::sleep(Duration::from_millis(20)).await;
        clock.send_replace(Clock::Finished);
        let report = task.await.unwrap();

        // Bob was liquidated once: after that, his health factor is above 1.
        assert_eq!(report.liquidations, 1);
        assert!(
            state.lock().position(&UserId::new("bob")).unwrap().debt < UsdcAmount::from_usdc(1_500)
        );
    }
}
