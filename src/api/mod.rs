//! HTTP API of CrashLend: the router and the state shared by every handler.
//!
//! Each part of the project adds its own routes in a submodule.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use axum::Json;
use axum::Router;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;

use crate::error::LendingError;
use crate::guardian::Guardian;
use crate::protocol::Protocol;
use crate::replay::ReplayState;

pub mod crash;
pub mod lending;
pub mod risk;

/// State shared by all the HTTP handlers.
///
/// It is cloned for every request, which only copies the `Arc` pointers.
/// When several locks are needed, they are always taken in the same order:
/// protocol, then guardian, then replay. Two tasks can therefore never wait
/// on each other.
#[derive(Clone)]
pub struct AppState {
    /// The lending pool. The replay task and the bots share the same pointer.
    pub protocol: Arc<Mutex<Protocol>>,
    /// The risk guardian, fed with each new price.
    pub guardian: Arc<Mutex<Guardian>>,
    /// Progress, liquidations and report of the crash replays.
    pub replay: Arc<Mutex<ReplayState>>,
    /// File used by `/save` and `/load`.
    pub state_file: PathBuf,
}

/// Name of the file used to save the state, at the root of the project.
pub const STATE_FILE: &str = "state.json";

/// Returns the default file used by `/save` and `/load`.
///
/// The path starts from the root of the project, known when the code is
/// compiled, and not from the folder the server is launched from. A state
/// saved by one server is therefore found by the next one, wherever it starts.
pub fn default_state_file() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(STATE_FILE)
}

impl AppState {
    /// Wraps a protocol so it can be shared between tasks. The guardian starts
    /// enabled, and the state is saved to `default_state_file()`.
    pub fn new(protocol: Protocol) -> AppState {
        AppState {
            protocol: Arc::new(Mutex::new(protocol)),
            guardian: Arc::new(Mutex::new(Guardian::new(true))),
            replay: Arc::new(Mutex::new(ReplayState::default())),
            state_file: default_state_file(),
        }
    }

    /// Locks the protocol. Keep the guard for a short operation only, and
    /// never across an `.await`.
    pub fn lock(&self) -> MutexGuard<'_, Protocol> {
        // A poisoned lock means a thread panicked while holding it. The state
        // of the pool is still readable, so we keep serving it.
        self.protocol
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Locks the guardian. Same rules as `lock`, and always after it.
    pub fn lock_guardian(&self) -> MutexGuard<'_, Guardian> {
        self.guardian
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Locks the replay state. Same rules as `lock`, and always last.
    pub fn lock_replay(&self) -> MutexGuard<'_, ReplayState> {
        self.replay
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// Builds the router with every route of the API.
pub fn router(state: AppState) -> Router {
    Router::new()
        .merge(lending::routes())
        .merge(risk::routes())
        .merge(crash::routes())
        .with_state(state)
}

/// Body of every error response: `{ "error": "<message>" }`.
#[derive(Debug, Serialize)]
pub struct ErrorBody {
    /// Readable description of the error.
    pub error: String,
}

/// Returns the HTTP status that matches an error.
pub fn status_of(error: &LendingError) -> StatusCode {
    match error {
        LendingError::ZeroAmount | LendingError::TooManyScenarios(_) => StatusCode::BAD_REQUEST,
        LendingError::UnknownUser(_) | LendingError::ScenarioNotFound(_) => StatusCode::NOT_FOUND,
        // The request is valid, but the rules of the protocol refuse it.
        LendingError::InsufficientCollateral
        | LendingError::InsufficientLiquidity
        | LendingError::NothingToRepay
        | LendingError::PositionHealthy
        | LendingError::NothingToLiquidate
        | LendingError::BorrowingPaused => StatusCode::UNPROCESSABLE_ENTITY,
        LendingError::ReplayAlreadyRunning => StatusCode::CONFLICT,
        LendingError::Storage(_) => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

/// Lets handlers return `Result<_, LendingError>` directly.
impl IntoResponse for LendingError {
    fn into_response(self) -> Response {
        let body = ErrorBody {
            error: self.to_string(),
        };
        (status_of(&self), Json(body)).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Params, Price, UserId};

    #[test]
    fn state_file_does_not_depend_on_the_launch_folder() {
        let state = AppState::new(Protocol::new(Params::default(), Price::from_usdc(2_000)));
        // An absolute path points to the same file wherever the server starts.
        assert!(state.state_file.is_absolute());
        assert_eq!(
            state.state_file,
            Path::new(env!("CARGO_MANIFEST_DIR")).join("state.json")
        );
    }

    #[test]
    fn errors_have_a_matching_status() {
        assert_eq!(
            status_of(&LendingError::ZeroAmount),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            status_of(&LendingError::UnknownUser(UserId::new("bob"))),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            status_of(&LendingError::TooManyScenarios(100_000)),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            status_of(&LendingError::BorrowingPaused),
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }

    #[test]
    fn error_response_uses_the_status() {
        let response = LendingError::InsufficientLiquidity.into_response();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }
}
