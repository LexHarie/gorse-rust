// Ports: server/*.go @ add21b5

//! The server node: REST API and online recommendation.
//!
//! [`server`] holds the node's life cycle (`server/server.go`: `Serve`, `Sync`,
//! `Shutdown`). [`rest`] holds the REST API (`server/rest.go`), and [`metrics`]
//! the Prometheus histogram of its requests (`server/metrics.go`).

pub mod metrics;
pub mod rest;
pub mod server;
