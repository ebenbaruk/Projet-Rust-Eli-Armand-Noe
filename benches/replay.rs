//! Benchmark of a full replay of Black Thursday, without delay between steps.
//!
//! It measures the work done at each of the 1 440 simulated minutes: setting
//! the price, adding interest to every position and informing the guardian.
//! The bots are left out: they sleep for a fixed time, so with them the
//! benchmark would measure the clock rather than the code.

use std::time::Duration;

use crashlend::api::AppState;
use crashlend::protocol::Protocol;
use crashlend::replay::{self, ReplayConfig};
use crashlend::scenario;
use crashlend::types::{EthAmount, Params, Price, UsdcAmount, UserId};
use criterion::{BatchSize, Criterion, criterion_group, criterion_main};

/// Number of borrowers in the replayed pool.
const BORROWERS: u64 = 1_000;

/// Builds a pool where each borrower uses a different share of their limit,
/// between 30 and 70 % of the value of 1 ETH.
fn pool() -> AppState {
    let mut protocol = Protocol::new(Params::default(), Price::from_usdc(2_000));
    protocol
        .supply(&UserId::new("lender"), UsdcAmount::from_usdc(10_000_000))
        .unwrap();
    for i in 0..BORROWERS {
        let user = UserId(format!("user-{i}"));
        protocol.deposit(&user, EthAmount::from_eth(1)).unwrap();
        let debt = UsdcAmount::from_usdc(600 + (i % 9) * 100);
        protocol.borrow(&user, debt).unwrap();
    }
    AppState::new(protocol)
}

fn replay_benchmark(c: &mut Criterion) {
    let scenario = scenario::load(&scenario::data_dir(), "black-thursday").unwrap();
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let config = ReplayConfig {
        step_delay: Duration::ZERO,
        bots: Vec::new(),
    };

    c.bench_function("replay_black_thursday_1000_positions", |b| {
        // A replay changes the pool, so each run starts from a fresh one.
        // Building it is not measured.
        b.iter_batched(
            pool,
            |state| {
                runtime.block_on(async {
                    let prices = scenario.prices.clone();
                    replay::start(&state, "black-thursday", prices, config.clone())
                        .unwrap()
                        .await
                        .unwrap()
                })
            },
            BatchSize::LargeInput,
        )
    });
}

criterion_group!(benches, replay_benchmark);
criterion_main!(benches);
