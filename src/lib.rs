//! Library surface for the doc-linter crate.
//!
//! Until this file existed, doc-linter was a binary-only crate: every
//! module lived under `main.rs` and the only way to drive the pipeline
//! was through the CLI. That made it impossible for external tooling
//! (the planned MCP server, embedded callers, third-party integration
//! tests) to reuse the pipeline without re-implementing the clap
//! dispatcher or shelling out.
//!
//! Re-publishing the same modules as `pub mod`s here turns the crate
//! into a lib + bin: `cargo build` now produces both `libdoc_linter.rlib`
//! and the `doc-linter` binary, and downstream code can
//! `use doc_linter::query::run` (or any other module) directly.
//! `src/main.rs` is a thin clap dispatcher that imports from this
//! library — it no longer declares the modules itself.
//!
//! Module groupings (the directory structure will follow in #47–#50):
//!
//! - **Pipeline core:** [`parser`], [`validator`], [`graph`],
//!   [`ontology`], [`disambiguation`].
//! - **Doc-comment extractors:** [`code_comments`], [`code_comments_py`],
//!   [`code_comments_ts`].
//! - **Config + coverage:** [`config`], [`coverage`].
//! - **Endpoint discovery:** [`endpoint_extract`],
//!   [`endpoint_markers_migrate`].
//! - **Graph store + query:** [`store`], [`query`], [`scip_ingest`].
//! - **Scaffolding + maintenance:** [`init`], [`scaffold`], [`promote`],
//!   [`homepage`], [`explain`], [`vale`].
//! - **Agent / editor surface:** [`lsp`].

pub mod code_comments;
mod code_comments_java;
pub mod code_comments_py;
mod code_comments_slash;
pub mod code_comments_ts;
pub mod comment_lint;
pub mod config;
pub mod coverage;
pub mod disambiguation;
pub mod embeddings;
pub mod endpoint_extract;
pub mod endpoint_markers_migrate;
pub mod explain;
pub mod gitdate;
pub mod graph;
pub mod graph_read;
pub mod homepage;
pub mod ids;
pub mod init;
pub mod llm;
pub mod lsp;
pub mod ontology;
pub mod parser;
pub mod promote;
pub mod query;
pub mod scaffold;
pub mod scip_ingest;
pub mod scip_ingest_cache;
pub mod store;
pub mod store_sqlite;
pub mod vale;
pub mod validator;
