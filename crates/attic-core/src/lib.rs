//! The parts of attic that don't need a terminal or launchd: configuration,
//! the store, scanning, applying file system events, suggestions, trashing,
//! notification policy and the service loop.

#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]
#![warn(missing_docs)]

pub mod apply;
pub mod config;
pub mod disk;
pub mod error;
pub mod events;
pub mod fsevents;
pub mod notify;
pub mod paths;
pub mod probe;
pub mod scan;
pub mod service;
pub mod store;
pub mod suggest;
pub mod trash;
pub mod units;

pub use config::Config;
pub use error::{AtticError, Result};
pub use paths::Paths;
