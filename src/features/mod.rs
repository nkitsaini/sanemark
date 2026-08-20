//! One module per LSP feature. Each is written as a pure function of
//! `(analysis / rope, config, ...)` so it can be unit-tested without an LSP
//! connection; [`crate::server`] simply adapts protocol requests to them.

pub mod completion;
pub mod diagnostics;
pub mod document_link;
pub mod folding;
pub mod formatting;
pub mod hover;
pub mod lists;
pub mod navigation;
pub mod snippets;
pub mod symbols;
pub mod tables;
