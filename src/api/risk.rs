//! Routes of the risk guardian and of the stress test.

use axum::extract::State;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::api::AppState;
use crate::error::LendingError;
use crate::guardian::{Guardian, RiskMode};
use crate::report::StressReport;
use crate::stress_test::{self, MAX_SCENARIOS, StressConfig};
use crate::types::Bps;

/// Routes of the risk part of the API.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/risk", get(get_risk))
        .route("/risk/toggle", post(toggle_risk))
        .route("/stress-test", post(run_stress_test))
}

/// Body of `POST /stress-test`: `{ "scenarios": 10000 }`.
#[derive(Debug, Deserialize)]
pub struct StressRequest {
    /// Number of random crashes to replay.
    pub scenarios: usize,
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

/// POST /stress-test: replays random crashes with and without the guardian.
/// Returns the report, or `ZeroAmount` for zero scenarios.
pub async fn run_stress_test(
    State(state): State<AppState>,
    Json(request): Json<StressRequest>,
) -> Result<Json<StressReport>, LendingError> {
    if request.scenarios == 0 {
        return Err(LendingError::ZeroAmount);
    }
    // A blocking task cannot be cancelled, so a huge request would keep every
    // core busy long after the client gave up.
    if request.scenarios > MAX_SCENARIOS {
        return Err(LendingError::TooManyScenarios(MAX_SCENARIOS));
    }
    let params = state.lock().params;
    let config = StressConfig::new(request.scenarios);
    // Seconds of pure computation: on the blocking pool, the async workers
    // keep serving the other requests in the meantime.
    let task = tokio::task::spawn_blocking(move || stress_test::run(params, &config));
    match task.await {
        Ok(report) => Ok(Json(report?)),
        // A blocking task cannot be cancelled, so the only error is a panic: pass it on.
        Err(error) => std::panic::resume_unwind(error.into_panic()),
    }
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

    #[tokio::test]
    async fn stress_test_refuses_a_bad_number_of_scenarios() {
        let state = test_state();

        let zero = Json(StressRequest { scenarios: 0 });
        let result = run_stress_test(State(state.clone()), zero).await;
        assert_eq!(result.err(), Some(LendingError::ZeroAmount));

        let too_many = Json(StressRequest {
            scenarios: MAX_SCENARIOS + 1,
        });
        let result = run_stress_test(State(state.clone()), too_many).await;
        assert_eq!(
            result.err(),
            Some(LendingError::TooManyScenarios(MAX_SCENARIOS))
        );
    }
}
