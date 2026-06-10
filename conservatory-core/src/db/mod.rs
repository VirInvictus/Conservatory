//! SQLite worker, read pool, and schema migrations (spec §2.1, §4).

mod command;
mod connection;
pub mod fixtures;
pub mod migrations;
pub mod models;
pub mod pool;
mod probe;
pub mod reads;
mod worker;
mod writes;

#[cfg(test)]
mod fts_tests;

pub use migrations::CURRENT_VERSION;
pub use models::{Album, Artist, Genre, Track};
pub use pool::ReadPool;
pub use probe::probe_read;
pub use reads::{LibraryCounts, get_album, get_artist, get_track, library_counts, list_albums};
pub use worker::{WorkerHandle, spawn_worker};
