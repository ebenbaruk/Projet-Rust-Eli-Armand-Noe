//! HTTP API of CrashLend: the router and the state shared by every handler.
//!
//! Each part of the project adds its own routes in a submodule.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};

use axum::Json;
use axum::Router;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;

use crate::error::LendingError;
use crate::guardian::Guardian;
use crate::protocol::Protocol;

pub mod lending;
pub mod risk;

/// State shared by all the HTTP handlers.
///
/// It is cloned for every request, which only copies the `Arc` pointers.
/// When both locks are needed, the protocol is always locked first, so two
/// handlers can never wait on each other.
#[derive(Clone)]
pub struct AppState {
    /// The lending pool. The replay task and the bots share the same pointer.
    pub protocol: Arc<Mutex<Protocol>>,
    /// The risk guardian, fed with each new price.
    pub guardian: Arc<Mutex<Guardian>>,
    /// File used by `/save` and `/load`.
    pub state_file: PathBuf,
}

/// Default file used to save the state.
pub const STATE_FILE: &str = "state.json";

impl AppState {
    /// Wraps a protocol so it can be shared between tasks. The guardian starts enabled.
    pub fn new(protocol: Protocol) -> AppState {
        AppState {
            protocol: Arc::new(Mutex::new(protocol)),
            guardian: Arc::new(Mutex::new(Guardian::new(true))),
            state_file: PathBuf::from(STATE_FILE),
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
}

/// Builds the router with every route of the API.
pub fn router(state: AppState) -> Router {
    Router::new()
        .merge(lending::routes())
        .merge(risk::routes())
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
        LendingError::ZeroAmount => StatusCode::BAD_REQUEST,
        LendingError::UnknownUser(_) | LendingError::ScenarioNotFound(_) => StatusCode::NOT_FOUND,
        // The request is valid, but the rules of the protocol refuse it.
        LendingError::InsufficientCollateral
        | LendingError::InsufficientLiquidity
        | LendingError::NothingToRepay
        | LendingError::PositionHealthy
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
    use crate::types::UserId;

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
