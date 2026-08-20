//! A high-quality, configurable Markdown language server.
//!
//! The crate is split into small, transport-independent modules so that every
//! feature can be unit-tested without spinning up an actual LSP connection:
//!
//! * [`analysis`] parses a document into an owned, position-resolved summary.
//! * [`features`] contains one module per LSP feature, each a pure function of
//!   `(analysis, rope, config)`.
//! * [`server`] wires everything into a [`tower_lsp_server`] backend.

pub mod analysis;
pub mod cli;
pub mod clipboard;
pub mod config;
pub mod document;
pub mod encoding;
pub mod features;
pub mod fuzzy;
pub mod journal;
pub mod links;
pub mod server;
pub mod uri;
