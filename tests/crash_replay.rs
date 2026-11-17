//! Integration test of the crash replay: short crashes replayed with the
//! liquidation bots, and the real Black Thursday rescaled to the pool.

use std::collections::BTreeSet;
use std::path::Path;
use std::time::Duration;

use crashlend::api::AppState;
use crashlend::bots::Bot;
use crashlend::protocol::Protocol;
use crashlend::replay::{self, ReplayConfig};
use crashlend::scenario::{self, DATA_DIR};
use crashlend::types::{Bps, EthAmount, Params, Price, UsdcAmount, UserId};

/// A pool at 2 000 USDC per ETH. Bob, Carol and Dave each deposit 1 ETH and
/// borrow 1 400, 1 200 and 1 000 USDC: they become liquidatable below 1 750,
/// 1 500 and 1 250 USDC.
fn pool_with_three_borrowers() -> AppState {
    let mut protocol = Protocol::new(Params::default(), Price::from_usdc(2_000));
    protocol
        .supply(&UserId::new("alice"), UsdcAmount::from_usdc(100_000))
        .unwrap();
    for (name, debt) in [("bob", 1_400), ("carol", 1_200), ("dave", 1_000)] {
        let user = UserId::new(name);
        protocol.deposit(&user, EthAmount::from_eth(1)).unwrap();
        protocol.borrow(&user, UsdcAmount::from_usdc(debt)).unwrap();
    }
    AppState::new(protocol)
}

/// The price falls by `step` USDC per minute from 2 000 USDC down to `low`,
/// then stays at `low` for `flat` minutes.
fn crash(step: u64, low: u64, flat: usize) -> Vec<Price> {
    let mut prices: Vec<Price> = (0..)
        .map(|minute| 2_000 - minute * step)
        .take_while(|&price| price > low)
        .map(Price::from_usdc)
        .collect();
    prices.extend(vec![Price::from_usdc(low); flat]);
    prices
}

#[tokio::test]
async fn bots_follow_a_progressive_crash() {
    let state = pool_with_three_borrowers();
    let config = ReplayConfig {
        step_delay: Duration::from_millis(2),
        bots: vec![
            Bot {
                name: "fast",
                reaction: Duration::from_millis(1),
            },
            Bot {
                name: "slow",
                reaction: Duration::from_millis(5),
            },
        ],
    };
    // -45 %, about 1 % per minute: the bots have time to react.
    let prices = crash(20, 1_100, 30);

    let report = replay::start(&state, "progressive", prices, config)
        .unwrap()
        .await
        .unwrap();

    assert_eq!(report.max_drop, Bps(4_500));
    // Every borrower was reached in time, so the pool lost nothing.
    assert_eq!(report.bad_debt, UsdcAmount(0));
    let replay = state.lock_replay();
    let liquidated: BTreeSet<&str> = replay
        .liquidations
        .iter()
        .map(|record| record.liquidation.user.0.as_str())
        .collect();
    assert_eq!(liquidated, BTreeSet::from(["bob", "carol", "dave"]));
    // The history and the report tell the same story.
    assert_eq!(replay.liquidations.len() as u64, report.liquidations);
    let repaid: u64 = replay
        .liquidations
        .iter()
        .map(|record| record.liquidation.repaid.0)
        .sum();
    assert_eq!(UsdcAmount(repaid), report.repaid);
}

#[tokio::test]
async fn a_crash_faster_than_the_bots_leaves_bad_debt() {
    let state = pool_with_three_borrowers();
    // The only bot reacts in 1 second, but the crash lasts a few milliseconds.
    let config = ReplayConfig {
        step_delay: Duration::from_millis(1),
        bots: vec![Bot {
            name: "too-slow",
            reaction: Duration::from_secs(1),
        }],
    };
    // -60 % in 12 minutes.
    let prices = crash(100, 800, 1);

    let report = replay::start(&state, "flash-crash", prices, config)
        .unwrap()
        .await
        .unwrap();

    assert_eq!(report.liquidations, 0);
    assert!(report.bad_debt > UsdcAmount(0));
}

#[tokio::test]
async fn black_thursday_is_rescaled_to_the_pool() {
    let state = pool_with_three_borrowers();
    let data = Path::new(env!("CARGO_MANIFEST_DIR")).join(DATA_DIR);
    let scenario = scenario::load(&data, "black-thursday").unwrap();
    let config = ReplayConfig {
        step_delay: Duration::ZERO,
        bots: Vec::new(),
    };

    let report = replay::start(&state, "black-thursday", scenario.prices, config)
        .unwrap()
        .await
        .unwrap();

    // The real crash went from 195.02 down to 101.37 USDC: -48 %.
    assert_eq!(report.minutes, 24 * 60);
    assert_eq!(report.start_price, Price::from_usdc(2_000));
    assert_eq!(report.max_drop, Bps(4_802));
    // Without any liquidator, Bob's 1 ETH ends below his debt.
    assert!(report.bad_debt > UsdcAmount(0));
}
