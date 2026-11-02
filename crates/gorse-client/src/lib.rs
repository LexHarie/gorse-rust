// Ports: client/admin.go @ add21b5, client/config.go @ add21b5

//! The admin API client (Go's `client.AdminClient`) and the SDK calls that
//! `gorse-cli` makes against the master's public routes.
//!
//! The SDK itself (`github.com/gorse-io/gorse-go`) is not ported here; its suite
//! runs under WP-4-7.

pub mod admin;
pub mod config;
pub mod model;
pub mod url;

pub use admin::{AdminClient, ClientError};
pub use config::CONFIG_TOML;
pub use model::{
    DumpStats, Feedback, FeedbackIterator, FeedbackKey, Item, ItemIterator, Node, Progress,
    ScoreUser, ScoredItem, Status, TimeSeriesPoint, User, UserIterator,
};
