//! Routes of the crash replays: scenarios, replay, liquidation history and report.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Serialize;

use crate::api::AppState;
use crate::error::LendingError;
use crate::replay::{self, LiquidationRecord, ReplayConfig};
use crate::report::ReplayReport;
use crate::scenario::{self, Scenario};
use crate::types::Price;

/// Routes of the crash part of the API.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/scenarios", get(list_scenarios))
        .route("/replay/{id}", post(start_replay))
        .route("/liquidations", get(list_liquidations))
        .route("/report", get(get_report))
}

/// A scenario as listed by the API, without its prices.
#[derive(Debug, Serialize)]
pub struct ScenarioView {
    /// Id to use in `POST /replay/{id}`.
    pub id: String,
    /// Readable name.
    pub name: String,
    /// Date and time of the first price.
    pub start: String,
    /// Number of simulated minutes.
    pub minutes: usize,
    /// Real price of ETH at the first minute. The replay rescales the
    /// scenario so that it starts at the current price of the pool.
    pub start_price: Price,
}

impl ScenarioView {
    fn new(scenario: &Scenario) -> ScenarioView {
        ScenarioView {
            id: scenario.id.clone(),
            name: scenario.name.clone(),
            start: scenario.start.clone(),
            minutes: scenario.prices.len(),
            start_price: scenario.prices.first().copied().unwrap_or_default(),
        }
    }
}

/// Answer of `POST /replay/{id}`: the replay runs in the background.
#[derive(Debug, Serialize)]
pub struct ReplayStarted {
    /// Id of the replayed scenario.
    pub scenario: String,
    /// Number of simulated minutes.
    pub minutes: usize,
}

/// GET /scenarios: lists the historical crashes and their real starting price.
pub async fn list_scenarios() -> Result<Json<Vec<ScenarioView>>, LendingError> {
    let scenarios = scenario::load_all(&scenario::data_dir())?;
    Ok(Json(scenarios.iter().map(ScenarioView::new).collect()))
}

/// POST /replay/{id}: starts the replay of a scenario with the liquidation bots.
///
/// The scenario is rescaled so that it starts at the current price. Answers
/// 202 right away, while the replay runs. Fails with `ScenarioNotFound` or
/// `ReplayAlreadyRunning`.
pub async fn start_replay(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<(StatusCode, Json<ReplayStarted>), LendingError> {
    let scenario = scenario::load(&scenario::data_dir(), &id)?;
    let minutes = scenario.prices.len();
    // The task is not awaited: it stores its report in the shared state.
    replay::start(&state, &id, scenario.prices, ReplayConfig::default())?;
    let started = ReplayStarted {
        scenario: id,
        minutes,
    };
    Ok((StatusCode::ACCEPTED, Json(started)))
}

/// GET /liquidations: liquidations of the current or last replay, oldest first.
pub async fn list_liquidations(State(state): State<AppState>) -> Json<Vec<LiquidationRecord>> {
    Json(state.lock_replay().liquidations.clone())
}

/// GET /report: report of the last finished replay, or `null` if none has finished yet.
pub async fn get_report(State(state): State<AppState>) -> Json<Option<ReplayReport>> {
    Json(state.lock_replay().report.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::liquidation::Liquidation;
    use crate::protocol::Protocol;
    use crate::types::{EthAmount, Params, UsdcAmount, UserId};

    /// Price of ETH at the first minute of Black Thursday, in `data/`.
    const BLACK_THURSDAY_START: Price = Price(19_502);

    fn test_state() -> AppState {
        AppState::new(Protocol::new(Params::default(), Price::from_usdc(2_000)))
    }

    fn black_thursday() -> Path<String> {
        Path("black-thursday".to_string())
    }

    #[tokio::test]
    async fn scenarios_are_listed_with_their_starting_price() {
        let Json(scenarios) = list_scenarios().await.unwrap();

        assert_eq!(scenarios.len(), 3);
        assert_eq!(scenarios[0].id, "black-thursday");
        assert_eq!(scenarios[0].minutes, 24 * 60);
        assert_eq!(scenarios[0].start_price, BLACK_THURSDAY_START);
    }

    #[tokio::test]
    async fn replay_of_an_unknown_scenario_is_refused() {
        let state = test_state();

        let result = start_replay(State(state), Path("covid".to_string())).await;

        assert_eq!(
            result.err(),
            Some(LendingError::ScenarioNotFound("covid".to_string()))
        );
    }

    #[tokio::test]
    async fn replay_starts_in_the_background() {
        // The pool is at 2 000 USDC, far from the 195 USDC of the scenario:
        // the replay is rescaled instead of refused.
        let state = test_state();

        let (status, Json(started)) = start_replay(State(state.clone()), black_thursday())
            .await
            .unwrap();

        assert_eq!(status, StatusCode::ACCEPTED);
        assert_eq!(started.minutes, 24 * 60);
        assert!(state.lock_replay().running);
        // A second replay cannot start while the first one runs.
        let second = start_replay(State(state.clone()), black_thursday()).await;
        assert_eq!(second.err(), Some(LendingError::ReplayAlreadyRunning));
    }

    #[tokio::test]
    async fn history_and_report_come_from_the_replay_state() {
        let state = test_state();
        let Json(report) = get_report(State(state.clone())).await;
        assert_eq!(report, None);

        let record = LiquidationRecord {
            minute: 12,
            bot: "fast",
            liquidation: Liquidation {
                user: UserId::new("bob"),
                repaid: UsdcAmount::from_usdc(750),
                seized: EthAmount::from_eth(1),
            },
        };
        state.lock_replay().liquidations.push(record.clone());

        let Json(history) = list_liquidations(State(state)).await;
        assert_eq!(history, vec![record]);
    }
}
