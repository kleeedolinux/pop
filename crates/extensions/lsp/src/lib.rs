//! Build metadata for the independently installed `Pop.Lsp` Package.

pub const PACKAGE: &str = "Pop.Lsp";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const NAMESPACES: &[&str] = &["Pop.Lsp"];
pub const DEPENDENCIES: &[&str] = &[pop_extension_rpc::PACKAGE, pop_extension_syntax::PACKAGE];
