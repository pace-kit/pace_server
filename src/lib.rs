//! Durable, explicitly versioned project snapshots. This is not a co-editing service.
pub mod api;
pub mod auth;
pub mod cors;
pub mod error;
pub mod project;
pub mod storage;

pub use api::router;
pub use auth::{authenticated_router, Auth};
pub use storage::{Principal, Store};
