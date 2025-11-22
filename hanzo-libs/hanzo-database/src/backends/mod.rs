//! Database backend implementations

#[cfg(feature = "backend-lancedb")]
pub mod lancedb;

#[cfg(feature = "backend-duckdb")]
pub mod duckdb;

#[cfg(feature = "backend-postgres")]
pub mod postgres;

#[cfg(feature = "backend-redis")]
pub mod redis;

#[cfg(feature = "backend-sqlite")]
pub mod sqlite;