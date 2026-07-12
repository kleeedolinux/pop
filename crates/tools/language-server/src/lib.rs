//! Incremental language-server implementation.
//!
//! Public protocol types belong to the independently installed `Pop.Lsp`
//! Package. Compiler/query integration remains private to this tool crate.

pub const PUBLIC_PROTOCOL_PACKAGE: &str = pop_extension_lsp::PACKAGE;
