//! HTTP API of CrashLend: the router and the state shared by every handler.
//!
//! Each part of the project adds its own routes in a submodule.

use std::sync::{Arc, Mutex, MutexGuard};

use axum::Router;

use crate::protocol::Protocol;

pub mod lending;

/// State shared by all the HTTP handlers.
///
/// It is cloned for every request, which only copies the `Arc` pointers.
#[derive(Clone)]
pub struct AppState {
    /// The lending pool. The replay task and the bots share the same pointer.
    pub protocol: Arc<Mutex<Protocol>>,
}

impl AppState {
    /// Wraps a protocol so it can be shared between tasks.
    pub fn new(protocol: Protocol) -> AppState {
        AppState {
            protocol: Arc::new(Mutex::new(protocol)),
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
}

/// Builds the router with every route of the API.
pub fn router(state: AppState) -> Router {
    Router::new().merge(lending::routes()).with_state(state)
}
