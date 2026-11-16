//! Integration test of the stress test: crashes whose outcome is known, and a
//! small random run whose result must not depend on the number of threads.

use crashlend::stress_test::{self, Portfolio, START_PRICE, StressConfig, simulate};
use crashlend::types::{Params, Price, UsdcAmount};

/// Straight fall of `drop_bps` from `START_PRICE` over `minutes`, without noise.
fn straight_crash(drop_bps: u64, minutes: u64) -> Vec<Price> {
    (1..=minutes)
        .map(|minute| Price(START_PRICE.0 * (10_000 - drop_bps * minute / minutes) / 10_000))
        .collect()
}

#[test]
fn slow_decline_is_survived_without_help() {
    let portfolio = Portfolio::new(Params::default()).unwrap();
    // -30 % over a day: the liquidators keep up.
    let prices = straight_crash(3_000, 24 * 60);
    assert_eq!(simulate(&portfolio, &prices, false), UsdcAmount(0));
    assert_eq!(simulate(&portfolio, &prices, true), UsdcAmount(0));
}

#[test]
fn guardian_saves_the_pool_from_a_fast_crash() {
    let portfolio = Portfolio::new(Params::default()).unwrap();
    // -45 % in 80 minutes: without the guardian, dip buyers borrow at 75 % LTV
    // on the way down and the liquidators cannot keep up.
    let prices = straight_crash(4_500, 80);
    assert!(simulate(&portfolio, &prices, false) > UsdcAmount(0));
    assert_eq!(simulate(&portfolio, &prices, true), UsdcAmount(0));
}

#[test]
fn result_does_not_depend_on_the_number_of_threads() {
    let config = |threads| StressConfig {
        scenarios: 40,
        threads,
        seed: 7,
    };
    let one = stress_test::run(Params::default(), &config(1)).unwrap();
    let four = stress_test::run(Params::default(), &config(4)).unwrap();

    assert_eq!(one.scenarios, 40);
    assert_eq!(
        one.survival_rate_without_guardian,
        four.survival_rate_without_guardian
    );
    assert_eq!(
        one.survival_rate_with_guardian,
        four.survival_rate_with_guardian
    );
    assert_eq!(
        one.average_bad_debt_without_guardian,
        four.average_bad_debt_without_guardian
    );
    assert_eq!(
        one.average_bad_debt_with_guardian,
        four.average_bad_debt_with_guardian
    );
}
