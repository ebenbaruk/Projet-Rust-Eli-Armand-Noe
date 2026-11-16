//! Routes of the risk guardian.

use axum::extract::State;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Serialize;

use crate::api::AppState;
use crate::guardian::{Guardian, RiskMode};
use crate::types::Bps;

/// Routes of the risk part of the API.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/risk", get(get_risk))
        .route("/risk/toggle", post(toggle_risk))
}

/// State of the guardian as returned by the API.
#[derive(Debug, Serialize)]
pub struct RiskView {
    /// True when the guardian restricts new borrows.
    pub enabled: bool,
    /// Mode applied to new borrows.
    pub mode: RiskMode,
    /// Drop of the price over the last hour.
    pub price_drop: Bps,
}

/// Builds the view of the guardian.
pub fn risk_view(guardian: &Guardian) -> RiskView {
    RiskView {
        enabled: guardian.is_enabled(),
        mode: guardian.mode(),
        price_drop: guardian.price_drop(),
    }
}

/// GET /risk: returns the mode of the guardian and the observed drop.
pub async fn get_risk(State(state): State<AppState>) -> Json<RiskView> {
    Json(risk_view(&state.lock_guardian()))
}

/// POST /risk/toggle: turns the guardian on or off. Returns its new state.
pub async fn toggle_risk(State(state): State<AppState>) -> Json<RiskView> {
    let mut protocol = state.lock();
    let mut guardian = state.lock_guardian();
    let enabled = !guardian.is_enabled();
    guardian.set_enabled(enabled, &mut protocol);
    Json(risk_view(&guardian))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::lending::{PriceRequest, set_price};
    use crate::protocol::Protocol;
    use crate::types::{Params, Price};

    fn test_state() -> AppState {
        AppState::new(Protocol::new(Params::default(), Price::from_usdc(2_000)))
    }

    async fn set(state: &AppState, usdc: u64) {
        let request = Json(PriceRequest {
            price: Price::from_usdc(usdc),
        });
        let _ = set_price(State(state.clone()), request).await.unwrap();
    }

    #[tokio::test]
    async fn price_crash_raises_the_mode() {
        let state = test_state();
        set(&state, 2_000).await;
        set(&state, 1_600).await;

        let Json(view) = get_risk(State(state.clone())).await;
        assert_eq!(view.mode, RiskMode::Emergency);
        assert_eq!(view.price_drop, Bps(2_000));
        assert!(state.lock().borrowing_paused);
    }

    #[tokio::test]
    async fn toggle_lifts_and_restores_the_restrictions() {
        let state = test_state();
        set(&state, 2_000).await;
        set(&state, 1_600).await;

        let Json(view) = toggle_risk(State(state.clone())).await;
        assert!(!view.enabled);
        assert_eq!(view.mode, RiskMode::Normal);
        assert!(!state.lock().borrowing_paused);

        let Json(view) = toggle_risk(State(state.clone())).await;
        assert_eq!(view.mode, RiskMode::Emergency);
        assert!(state.lock().borrowing_paused);
    }
}
