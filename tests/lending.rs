//! Integration test of the lending protocol: a full cycle of supply, deposit,
//! borrow, interest, repay and withdraw, then a price crash and a save/load.

use crashlend::error::LendingError;
use crashlend::persistence;
use crashlend::protocol::Protocol;
use crashlend::types::{EthAmount, Params, Price, UsdcAmount, UserId};

/// Minutes in one day, the length of a crash replay.
const ONE_DAY: u64 = 24 * 60;

#[test]
fn full_lending_cycle() {
    let alice = UserId::new("alice");
    let bob = UserId::new("bob");
    let mut protocol = Protocol::new(Params::default(), Price::from_usdc(2_000));

    // Alice funds the pool, Bob borrows against 2 ETH.
    protocol
        .supply(&alice, UsdcAmount::from_usdc(100_000))
        .unwrap();
    protocol.deposit(&bob, EthAmount::from_eth(2)).unwrap();
    protocol.borrow(&bob, UsdcAmount::from_usdc(2_000)).unwrap();

    // One day of interest makes the debt grow a little.
    let mut interest = 0;
    for _ in 0..ONE_DAY {
        interest += protocol.accrue_interest().0;
    }
    let debt = protocol.position(&bob).unwrap().debt;
    assert!(interest > 0);
    assert_eq!(debt.0, UsdcAmount::from_usdc(2_000).0 + interest);

    // Bob cannot take his ETH back while he owes money.
    assert_eq!(
        protocol.withdraw(&bob, EthAmount::from_eth(2)),
        Err(LendingError::InsufficientCollateral)
    );

    // He repays more than needed: only the debt is taken.
    let position = protocol.repay(&bob, UsdcAmount::from_usdc(3_000)).unwrap();
    assert_eq!(position.debt, UsdcAmount(0));
    let position = protocol.withdraw(&bob, EthAmount::from_eth(2)).unwrap();
    assert_eq!(position.collateral, EthAmount(0));

    // The pool got its money back, plus the interest.
    assert_eq!(protocol.cash.0, UsdcAmount::from_usdc(100_000).0 + interest);
}

#[test]
fn price_crash_creates_risk_and_bad_debt() {
    let bob = UserId::new("bob");
    let mut protocol = Protocol::new(Params::default(), Price::from_usdc(2_000));
    protocol
        .supply(&UserId::new("alice"), UsdcAmount::from_usdc(10_000))
        .unwrap();
    protocol.deposit(&bob, EthAmount::from_eth(1)).unwrap();
    protocol.borrow(&bob, UsdcAmount::from_usdc(1_500)).unwrap();
    let threshold = protocol.params.liquidation_threshold;

    // At 2 000 USDC the position is safe.
    let position = protocol.position(&bob).unwrap();
    assert!(!position.is_liquidatable(protocol.price, threshold));

    // ETH drops 20 %: the position can be liquidated but is still covered.
    protocol.price = Price::from_usdc(1_600);
    assert!(position.is_liquidatable(protocol.price, threshold));
    assert_eq!(position.bad_debt(protocol.price), UsdcAmount(0));

    // ETH drops 40 %: the collateral no longer covers the debt.
    protocol.price = Price::from_usdc(1_200);
    assert_eq!(
        position.bad_debt(protocol.price),
        UsdcAmount::from_usdc(300)
    );
}

#[test]
fn saved_state_can_be_reloaded() {
    let mut protocol = Protocol::new(Params::default(), Price::from_usdc(2_000));
    protocol
        .supply(&UserId::new("alice"), UsdcAmount::from_usdc(10_000))
        .unwrap();
    protocol
        .deposit(&UserId::new("bob"), EthAmount::from_eth(1))
        .unwrap();
    protocol
        .borrow(&UserId::new("bob"), UsdcAmount::from_usdc(700))
        .unwrap();

    let path = std::env::temp_dir().join("crashlend-integration.json");
    persistence::save(&protocol, &path).unwrap();
    let loaded = persistence::load(&path).unwrap();
    std::fs::remove_file(&path).unwrap();

    assert_eq!(loaded, protocol);
}
