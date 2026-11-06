//! Routes of the lending protocol: supply, deposit, borrow, repay, withdraw
//! and the views of the pool and positions.

use axum::extract::State;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::api::AppState;
use crate::error::LendingError;
use crate::protocol::Protocol;
use crate::types::{Bps, EthAmount, HealthFactor, Position, Price, UsdcAmount, UserId};

/// Routes of the lending part of the API.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/supply", post(supply))
        .route("/deposit", post(deposit))
        .route("/borrow", post(borrow))
        .route("/repay", post(repay))
        .route("/withdraw", post(withdraw))
        .route("/pool", get(get_pool))
        .route("/positions", get(get_positions))
        .route("/price", get(get_price).post(set_price))
}

/// Body of the requests that move USDC: `{ "user": "alice", "amount": 100000 }`.
#[derive(Debug, Deserialize)]
pub struct UsdcRequest {
    /// User making the request.
    pub user: UserId,
    /// Amount in USDC cents.
    pub amount: UsdcAmount,
}

/// Body of the requests that move ETH: `{ "user": "bob", "amount": 2000000000 }`.
#[derive(Debug, Deserialize)]
pub struct EthRequest {
    /// User making the request.
    pub user: UserId,
    /// Amount in gwei.
    pub amount: EthAmount,
}

/// State of the pool as returned by the API.
#[derive(Debug, Serialize)]
pub struct PoolView {
    /// USDC available for new borrows.
    pub cash: UsdcAmount,
    /// Total debt of all borrowers.
    pub total_debt: UsdcAmount,
    /// Share of the pool lent out.
    pub utilization: Bps,
    /// Current yearly borrow rate.
    pub borrow_rate: Bps,
    /// Current price of 1 ETH.
    pub price: Price,
}

/// Position of a user as returned by the API.
#[derive(Debug, Serialize)]
pub struct PositionView {
    /// Owner of the position.
    pub user: UserId,
    /// ETH deposited as collateral.
    pub collateral: EthAmount,
    /// USDC owed, interest included.
    pub debt: UsdcAmount,
    /// Value of the collateral at the current price.
    pub collateral_value: UsdcAmount,
    /// Largest debt allowed for new borrows.
    pub max_borrow: UsdcAmount,
    /// Health factor scaled by 10 000, or `null` when there is no debt.
    pub health_factor: Option<HealthFactor>,
    /// True when the position can be liquidated.
    pub liquidatable: bool,
}

/// Body of `POST /price`: `{ "price": 150000 }`.
#[derive(Debug, Deserialize)]
pub struct PriceRequest {
    /// New price of 1 ETH in USDC cents.
    pub price: Price,
}

/// Current price of ETH.
#[derive(Debug, Serialize)]
pub struct PriceView {
    /// Price of 1 ETH in USDC cents.
    pub price: Price,
}

/// Builds the view of the pool.
pub fn pool_view(protocol: &Protocol) -> PoolView {
    PoolView {
        cash: protocol.cash,
        total_debt: protocol.total_debt(),
        utilization: protocol.utilization(),
        borrow_rate: protocol.current_rate(),
        price: protocol.price,
    }
}

/// Builds the view of one position.
pub fn position_view(protocol: &Protocol, user: &UserId, position: Position) -> PositionView {
    let threshold = protocol.params.liquidation_threshold;
    let health = position.health_factor(protocol.price, threshold);
    // An infinite health factor has no JSON number, so it is sent as null.
    let health_factor = if health == HealthFactor::INFINITE {
        None
    } else {
        Some(health)
    };
    PositionView {
        user: user.clone(),
        collateral: position.collateral,
        debt: position.debt,
        collateral_value: position.collateral_value(protocol.price),
        max_borrow: position.max_borrow(protocol.price, protocol.current_max_ltv()),
        health_factor,
        liquidatable: position.is_liquidatable(protocol.price, threshold),
    }
}

/// POST /supply: adds USDC to the pool. Returns the state of the pool.
pub async fn supply(
    State(state): State<AppState>,
    Json(request): Json<UsdcRequest>,
) -> Result<Json<PoolView>, LendingError> {
    let mut protocol = state.lock();
    protocol.supply(&request.user, request.amount)?;
    Ok(Json(pool_view(&protocol)))
}

/// POST /deposit: adds ETH collateral. Returns the position of the user.
pub async fn deposit(
    State(state): State<AppState>,
    Json(request): Json<EthRequest>,
) -> Result<Json<PositionView>, LendingError> {
    let mut protocol = state.lock();
    let position = protocol.deposit(&request.user, request.amount)?;
    Ok(Json(position_view(&protocol, &request.user, position)))
}

/// POST /borrow: lends USDC to the user. Returns the position of the user.
pub async fn borrow(
    State(state): State<AppState>,
    Json(request): Json<UsdcRequest>,
) -> Result<Json<PositionView>, LendingError> {
    let mut protocol = state.lock();
    let position = protocol.borrow(&request.user, request.amount)?;
    Ok(Json(position_view(&protocol, &request.user, position)))
}

/// POST /repay: pays back debt. Returns the position of the user.
pub async fn repay(
    State(state): State<AppState>,
    Json(request): Json<UsdcRequest>,
) -> Result<Json<PositionView>, LendingError> {
    let mut protocol = state.lock();
    let position = protocol.repay(&request.user, request.amount)?;
    Ok(Json(position_view(&protocol, &request.user, position)))
}

/// POST /withdraw: takes ETH collateral back. Returns the position of the user.
pub async fn withdraw(
    State(state): State<AppState>,
    Json(request): Json<EthRequest>,
) -> Result<Json<PositionView>, LendingError> {
    let mut protocol = state.lock();
    let position = protocol.withdraw(&request.user, request.amount)?;
    Ok(Json(position_view(&protocol, &request.user, position)))
}

/// GET /pool: returns the state of the pool.
pub async fn get_pool(State(state): State<AppState>) -> Json<PoolView> {
    let protocol = state.lock();
    Json(pool_view(&protocol))
}

/// GET /positions: returns every position with its health factor.
pub async fn get_positions(State(state): State<AppState>) -> Json<Vec<PositionView>> {
    let protocol = state.lock();
    let mut views = Vec::new();
    for (user, position) in &protocol.positions {
        views.push(position_view(&protocol, user, *position));
    }
    Json(views)
}

/// GET /price: returns the current price of ETH.
pub async fn get_price(State(state): State<AppState>) -> Json<PriceView> {
    let protocol = state.lock();
    Json(PriceView {
        price: protocol.price,
    })
}

/// POST /price: sets the price of ETH by hand, to test liquidations without
/// a replay. Returns the new price, or `ZeroAmount` for a zero price.
pub async fn set_price(
    State(state): State<AppState>,
    Json(request): Json<PriceRequest>,
) -> Result<Json<PriceView>, LendingError> {
    if request.price.0 == 0 {
        return Err(LendingError::ZeroAmount);
    }
    let mut protocol = state.lock();
    protocol.price = request.price;
    Ok(Json(PriceView {
        price: protocol.price,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Params;

    fn test_state() -> AppState {
        AppState::new(Protocol::new(Params::default(), Price::from_usdc(2_000)))
    }

    /// 10 000 USDC supplied by alice and 1 ETH deposited by bob.
    fn funded_state() -> AppState {
        let state = test_state();
        {
            let mut protocol = state.lock();
            protocol
                .supply(&UserId::new("alice"), UsdcAmount::from_usdc(10_000))
                .unwrap();
            protocol
                .deposit(&UserId::new("bob"), EthAmount::from_eth(1))
                .unwrap();
        }
        state
    }

    fn usdc(user: &str, usdc: u64) -> Json<UsdcRequest> {
        Json(UsdcRequest {
            user: UserId::new(user),
            amount: UsdcAmount::from_usdc(usdc),
        })
    }

    fn eth(user: &str, eth: u64) -> Json<EthRequest> {
        Json(EthRequest {
            user: UserId::new(user),
            amount: EthAmount::from_eth(eth),
        })
    }

    #[tokio::test]
    async fn price_route_returns_the_current_price() {
        let Json(view) = get_price(State(test_state())).await;
        assert_eq!(view.price, Price::from_usdc(2_000));
    }

    #[tokio::test]
    async fn supply_deposit_and_borrow() {
        let state = test_state();
        let Json(pool) = supply(State(state.clone()), usdc("alice", 10_000))
            .await
            .unwrap();
        assert_eq!(pool.cash, UsdcAmount::from_usdc(10_000));

        let Json(position) = deposit(State(state.clone()), eth("bob", 1)).await.unwrap();
        assert_eq!(position.health_factor, None);
        let Json(position) = borrow(State(state.clone()), usdc("bob", 1_000))
            .await
            .unwrap();
        assert_eq!(position.debt, UsdcAmount::from_usdc(1_000));
        // 2 000 x 80 % / 1 000 = 1.6
        assert_eq!(position.health_factor, Some(HealthFactor(16_000)));
        assert!(!position.liquidatable);
    }

    #[tokio::test]
    async fn borrow_errors_are_returned() {
        let state = test_state();
        let result = borrow(State(state), usdc("carol", 10)).await;
        assert!(matches!(result, Err(LendingError::UnknownUser(_))));
    }

    #[test]
    fn request_body_is_parsed_into_domain_types() {
        let json = r#"{ "user": "bob", "amount": 2000000000 }"#;
        let request: EthRequest = serde_json::from_str(json).unwrap();
        assert_eq!(request.user, UserId::new("bob"));
        assert_eq!(request.amount, EthAmount::from_eth(2));
    }

    #[tokio::test]
    async fn repay_and_withdraw() {
        let state = funded_state();
        state
            .lock()
            .borrow(&UserId::new("bob"), UsdcAmount::from_usdc(1_000))
            .unwrap();

        let Json(position) = repay(State(state.clone()), usdc("bob", 1_000))
            .await
            .unwrap();
        assert_eq!(position.debt, UsdcAmount(0));
        let Json(position) = withdraw(State(state.clone()), eth("bob", 1)).await.unwrap();
        assert_eq!(position.collateral, EthAmount(0));
    }

    #[tokio::test]
    async fn withdraw_errors_are_returned() {
        let state = funded_state();
        let result = withdraw(State(state), eth("bob", 2)).await;
        assert!(matches!(result, Err(LendingError::InsufficientCollateral)));
    }

    #[tokio::test]
    async fn pool_route_shows_utilization() {
        let state = funded_state();
        state
            .lock()
            .borrow(&UserId::new("bob"), UsdcAmount::from_usdc(1_000))
            .unwrap();
        let Json(pool) = get_pool(State(state)).await;
        assert_eq!(pool.total_debt, UsdcAmount::from_usdc(1_000));
        assert_eq!(pool.utilization, Bps(1_000));
    }

    #[tokio::test]
    async fn price_drop_makes_a_position_liquidatable() {
        let state = funded_state();
        state
            .lock()
            .borrow(&UserId::new("bob"), UsdcAmount::from_usdc(1_500))
            .unwrap();

        let request = Json(PriceRequest {
            price: Price::from_usdc(1_500),
        });
        let Json(view) = set_price(State(state.clone()), request).await.unwrap();
        assert_eq!(view.price, Price::from_usdc(1_500));

        let Json(positions) = get_positions(State(state)).await;
        assert_eq!(positions.len(), 1);
        assert!(positions[0].liquidatable);
    }

    #[tokio::test]
    async fn zero_price_is_refused() {
        let request = Json(PriceRequest { price: Price(0) });
        let result = set_price(State(test_state()), request).await;
        assert!(matches!(result, Err(LendingError::ZeroAmount)));
    }
}
