//! Stress test: thousands of random crashes, each replayed with and without the
//! risk guardian, to measure how often the pool ends without bad debt.
//!
//! The work is pure computation with no waiting, so the crashes are spread over
//! system threads with `std::thread::scope`. As an async task, it would block a
//! worker of the tokio runtime for the whole run.

use std::ops::{Bound, Range};
use std::thread;
use std::time::Instant;

use rand::rngs::StdRng;
use rand::{RngExt, SeedableRng};

use crate::error::LendingError;
use crate::guardian::Guardian;
use crate::liquidation;
use crate::protocol::Protocol;
use crate::report::{StressReport, Tally};
use crate::types::{BPS_SCALE, Bps, EthAmount, Params, Price, UsdcAmount, UserId, to_u64};

/// Price of 1 ETH at the start of every crash.
pub const START_PRICE: Price = Price(200_000);

/// Seed used by the API, so the same request always gives the same report.
pub const DEFAULT_SEED: u64 = 42;

/// Largest number of crashes the API accepts in one request. 10 000 crashes
/// take a few seconds, so this keeps a single request under about a minute.
pub const MAX_SCENARIOS: usize = 100_000;

/// Smallest and largest total drop of a random crash.
const MIN_DROP: Bps = Bps(1_000);
const MAX_DROP: Bps = Bps(7_000);

/// Shortest and longest random crash, in minutes.
const MIN_MINUTES: u64 = 30;
const MAX_MINUTES: u64 = 24 * 60;

/// Largest random move of the price around its trend, each minute.
const NOISE: Bps = Bps(50);

/// Liquidations the liquidators manage in one minute. Like on Black Thursday,
/// they cannot keep up when many positions become unhealthy at once.
const LIQUIDATIONS_PER_MINUTE: usize = 2;

/// Borrowers already in debt when the crash starts.
const BORROWERS: u64 = 80;

/// Borrowers who wait for the price to fall before borrowing the most they can.
const DIP_BUYERS: u64 = 20;

/// Settings of one stress test.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StressConfig {
    /// Number of random crashes.
    pub scenarios: usize,
    /// Number of threads the crashes are spread over.
    pub threads: usize,
    /// Seed of the random crashes. The same seed always gives the same crashes.
    pub seed: u64,
}

impl StressConfig {
    /// Uses every core of the machine and the default seed.
    pub fn new(scenarios: usize) -> StressConfig {
        let threads = thread::available_parallelism().map_or(1, |cores| cores.get());
        StressConfig {
            scenarios,
            threads,
            seed: DEFAULT_SEED,
        }
    }
}

/// The pool every crash starts from.
#[derive(Debug, Clone)]
pub struct Portfolio {
    protocol: Protocol,
    /// Each dip buyer and the price at which they borrow.
    dip_buyers: Vec<(UserId, Price)>,
}

impl Portfolio {
    /// Builds the same pool for every crash: borrowers between 30 % and 70 %
    /// LTV, and dip buyers waiting for drops of 2.5 % to 50 %.
    pub fn new(params: Params) -> Result<Portfolio, LendingError> {
        let mut protocol = Protocol::new(params, START_PRICE);
        protocol.supply(&UserId::new("lender"), UsdcAmount::from_usdc(10_000_000))?;

        for i in 0..BORROWERS {
            let user = UserId(format!("borrower-{i}"));
            let position = protocol.deposit(&user, EthAmount::from_eth(1 + i % 10))?;
            let ltv = Bps(3_000 + 4_000 * i / BORROWERS);
            protocol.borrow(&user, ltv.apply_to(position.collateral_value(START_PRICE)))?;
        }

        let mut dip_buyers = Vec::new();
        for i in 0..DIP_BUYERS {
            let user = UserId(format!("dip-buyer-{i}"));
            protocol.deposit(&user, EthAmount::from_eth(5))?;
            let drop = Bps(250 * (i + 1));
            dip_buyers.push((user, Price(scale(START_PRICE.0, BPS_SCALE - drop.0))));
        }

        Ok(Portfolio {
            protocol,
            dip_buyers,
        })
    }
}

/// Returns `value x bps / 10 000`.
fn scale(value: u64, bps: u64) -> u64 {
    to_u64(value as u128 * bps as u128 / BPS_SCALE as u128)
}

/// Generates a crash: a straight fall from `START_PRICE`, one price per minute,
/// with a small random move around the trend.
fn random_crash(rng: &mut StdRng) -> Vec<Price> {
    let drop = rng.random_range(MIN_DROP.0..=MAX_DROP.0);
    let minutes = rng.random_range(MIN_MINUTES..=MAX_MINUTES);
    (1..=minutes)
        .map(|minute| {
            let trend = scale(START_PRICE.0, BPS_SCALE - drop * minute / minutes);
            let noise = rng.random_range(0..=2 * NOISE.0);
            Price(scale(trend, BPS_SCALE - NOISE.0 + noise))
        })
        .collect()
}

/// Replays a crash on a copy of the portfolio and returns the bad debt left at the end.
///
/// Each minute: new price, interest, guardian, dip buyers, then liquidations.
pub fn simulate(portfolio: &Portfolio, prices: &[Price], guardian_enabled: bool) -> UsdcAmount {
    let mut protocol = portfolio.protocol.clone();
    let mut guardian = Guardian::new(guardian_enabled);

    for &price in prices {
        protocol.price = price;
        protocol.accrue_interest();
        guardian.observe(&mut protocol);
        for (user, trigger) in &portfolio.dip_buyers {
            if price <= *trigger {
                buy_the_dip(&mut protocol, user);
            }
        }
        liquidate(&mut protocol);
    }

    protocol.total_bad_debt()
}

/// A dip buyer without debt borrows the most the protocol allows right now.
fn buy_the_dip(protocol: &mut Protocol, user: &UserId) {
    let Ok(position) = protocol.position(user) else {
        return;
    };
    if position.debt.0 > 0 {
        return;
    }
    let amount = position.max_borrow(protocol.price, protocol.current_max_ltv());
    // A refusal, such as a pause from the guardian, is part of the simulation:
    // the buyer simply tries again the next minute.
    let _ = protocol.borrow(user, amount);
}

/// Liquidates the first unhealthy positions, in the order of the `BTreeMap`,
/// with the rule of section 4.5, up to `LIQUIDATIONS_PER_MINUTE`.
fn liquidate(protocol: &mut Protocol) {
    let mut liquidated = 0;
    let mut last_tried = None;
    while liquidated < LIQUIDATIONS_PER_MINUTE {
        let Some((user, debt)) = next_target(protocol, last_tried.as_ref()) else {
            return;
        };
        // A failed attempt does not use a slot: a real liquidator would not
        // send a liquidation that repays nothing (`NothingToLiquidate`).
        if liquidation::liquidate(protocol, &user, debt).is_ok() {
            liquidated += 1;
        }
        last_tried = Some(user);
    }
}

/// Returns the first unhealthy position after `after`, with its debt.
///
/// As for the bots, positions without collateral are skipped: there is
/// nothing left to seize.
fn next_target(protocol: &Protocol, after: Option<&UserId>) -> Option<(UserId, UsdcAmount)> {
    let price = protocol.price;
    let threshold = protocol.params.liquidation_threshold;
    let start = after.map_or(Bound::Unbounded, Bound::Excluded);
    protocol
        .positions
        .range((start, Bound::Unbounded))
        .find(|(_, position)| {
            position.collateral.0 > 0 && position.is_liquidatable(price, threshold)
        })
        .map(|(user, position)| (user.clone(), position.debt))
}

/// Replays the crashes of `indexes`, each with and without the guardian.
///
/// Each crash has its own seed, so the results do not depend on how the
/// crashes are split between threads.
fn run_range(portfolio: &Portfolio, indexes: Range<usize>, seed: u64) -> (Tally, Tally) {
    let mut without = Tally::default();
    let mut with = Tally::default();
    for index in indexes {
        let mut rng = StdRng::seed_from_u64(seed.wrapping_add(index as u64));
        let prices = random_crash(&mut rng);
        without.record(simulate(portfolio, &prices, false));
        with.record(simulate(portfolio, &prices, true));
    }
    (without, with)
}

/// Runs the whole stress test, spread over `config.threads` threads.
pub fn run(params: Params, config: &StressConfig) -> Result<StressReport, LendingError> {
    let started = Instant::now();
    let portfolio = Portfolio::new(params)?;
    let threads = config.threads.max(1);
    let per_thread = config.scenarios.div_ceil(threads);

    let (without, with) = thread::scope(|scope| {
        let handles: Vec<_> = (0..threads)
            .map(|t| {
                let start = (t * per_thread).min(config.scenarios);
                let end = (start + per_thread).min(config.scenarios);
                let portfolio = &portfolio;
                scope.spawn(move || run_range(portfolio, start..end, config.seed))
            })
            .collect();

        let mut total = (Tally::default(), Tally::default());
        for handle in handles {
            let (without, with) = handle
                .join()
                .unwrap_or_else(|panic| std::panic::resume_unwind(panic));
            total = (total.0.merge(without), total.1.merge(with));
        }
        total
    });

    Ok(StressReport::new(without, with, threads, started.elapsed()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn portfolio() -> Portfolio {
        Portfolio::new(Params::default()).unwrap()
    }

    #[test]
    fn crashes_stay_within_their_bounds() {
        let mut rng = StdRng::seed_from_u64(1);
        for _ in 0..100 {
            let prices = random_crash(&mut rng);
            let minutes = prices.len() as u64;
            assert!((MIN_MINUTES..=MAX_MINUTES).contains(&minutes));
            let last = prices[prices.len() - 1].0;
            // Bottom between -70 % and -10 %, give or take the noise.
            assert!(last >= scale(START_PRICE.0, 2_950));
            assert!(last <= scale(START_PRICE.0, 9_050));
        }
    }

    #[test]
    fn same_seed_gives_the_same_crash() {
        let first = random_crash(&mut StdRng::seed_from_u64(7));
        let second = random_crash(&mut StdRng::seed_from_u64(7));
        assert_eq!(first, second);
    }

    #[test]
    fn flat_price_leaves_no_bad_debt() {
        let prices = vec![START_PRICE; 120];
        assert_eq!(simulate(&portfolio(), &prices, false), UsdcAmount(0));
    }

    /// A pool at 1 500 USDC per ETH where each user owes 1 500 USDC against
    /// 1 ETH, so every position can be liquidated.
    fn unhealthy_pool(users: &[&str]) -> Protocol {
        let mut protocol = Protocol::new(Params::default(), Price::from_usdc(2_000));
        protocol
            .supply(&UserId::new("lender"), UsdcAmount::from_usdc(100_000))
            .unwrap();
        for name in users {
            let user = UserId::new(name);
            protocol.deposit(&user, EthAmount::from_eth(1)).unwrap();
            protocol
                .borrow(&user, UsdcAmount::from_usdc(1_500))
                .unwrap();
        }
        protocol.price = Price::from_usdc(1_500);
        protocol
    }

    #[test]
    fn positions_with_nothing_to_seize_do_not_take_a_slot() {
        let mut protocol = unhealthy_pool(&["dust", "empty", "user-1", "user-2", "user-3"]);
        // Left behind by liquidations that seized all the ETH, or all but
        // 1 gwei, which is worth less than a cent. Both names sort first.
        let dust = UserId::new("dust");
        let empty = UserId::new("empty");
        protocol.positions.get_mut(&dust).unwrap().collateral = EthAmount(1);
        protocol.positions.get_mut(&empty).unwrap().collateral = EthAmount(0);

        liquidate(&mut protocol);

        let debt = |name: &str| protocol.position(&UserId::new(name)).unwrap().debt;
        // Nobody repays debt that buys no collateral: the bad debt stays.
        assert_eq!(debt("dust"), UsdcAmount::from_usdc(1_500));
        assert_eq!(debt("empty"), UsdcAmount::from_usdc(1_500));
        // The two slots of the minute go to the next unhealthy positions.
        assert_eq!(debt("user-1"), UsdcAmount::from_usdc(750));
        assert_eq!(debt("user-2"), UsdcAmount::from_usdc(750));
        assert_eq!(debt("user-3"), UsdcAmount::from_usdc(1_500));
    }
}
