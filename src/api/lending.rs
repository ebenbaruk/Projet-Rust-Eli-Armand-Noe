//! Routes of the lending protocol: supply, deposit, borrow, repay, withdraw
//! and the views of the pool and positions.

use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;

use crate::api::AppState;
use crate::types::Price;

/// Routes of the lending part of the API.
pub fn routes() -> Router<AppState> {
    Router::new().route("/price", get(get_price))
}

/// Current price of ETH.
#[derive(Debug, Serialize)]
pub struct PriceView {
    /// Price of 1 ETH in USDC cents.
    pub price: Price,
}

/// GET /price: returns the current price of ETH.
pub async fn get_price(State(state): State<AppState>) -> Json<PriceView> {
    let protocol = state.lock();
    Json(PriceView {
        price: protocol.price,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::Protocol;
    use crate::types::Params;

    fn test_state() -> AppState {
        AppState::new(Protocol::new(Params::default(), Price::from_usdc(2_000)))
    }

    #[tokio::test]
    async fn price_route_returns_the_current_price() {
        let Json(view) = get_price(State(test_state())).await;
        assert_eq!(view.price, Price::from_usdc(2_000));
    }
}
