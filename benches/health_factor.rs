//! Benchmarks of the lending protocol on a pool of 10 000 positions.
//!
//! The bots check every health factor at each step of a replay, and interest is
//! added to every position each minute, so both loops run very often.

use std::hint::black_box;

use crashlend::protocol::Protocol;
use crashlend::types::{EthAmount, Params, Price, UsdcAmount, UserId};
use criterion::{Criterion, criterion_group, criterion_main};

/// Number of positions in the benchmarked pool.
const POSITIONS: u64 = 10_000;

/// Builds a pool where each user borrows a different share of their limit.
fn large_protocol() -> Protocol {
    let mut protocol = Protocol::new(Params::default(), Price::from_usdc(2_000));
    protocol
        .supply(&UserId::new("lender"), UsdcAmount::from_usdc(100_000_000))
        .unwrap();
    for i in 0..POSITIONS {
        let user = UserId(format!("user-{i}"));
        protocol.deposit(&user, EthAmount::from_eth(1)).unwrap();
        // Between 1 000 and 1 499 USDC, so the health factors differ.
        let debt = UsdcAmount::from_usdc(1_000 + i % 500);
        protocol.borrow(&user, debt).unwrap();
    }
    protocol
}

/// Counts the positions that can be liquidated, like a bot does at each step.
fn count_liquidatable(protocol: &Protocol) -> usize {
    let threshold = protocol.params.liquidation_threshold;
    let mut count = 0;
    for position in protocol.positions.values() {
        if position.is_liquidatable(protocol.price, threshold) {
            count += 1;
        }
    }
    count
}

fn health_factor_benchmark(c: &mut Criterion) {
    let mut protocol = large_protocol();
    // At this price about half of the positions are below a health factor of 1.
    protocol.price = Price::from_usdc(1_560);

    c.bench_function("health_factor_10000_positions", |b| {
        b.iter(|| count_liquidatable(black_box(&protocol)))
    });

    c.bench_function("accrue_interest_10000_positions", |b| {
        b.iter(|| black_box(&mut protocol).accrue_interest())
    });
}

criterion_group!(benches, health_factor_benchmark);
criterion_main!(benches);
