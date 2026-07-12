//! Build metadata for the independently installed `Pop.Rpc` Package.

pub const PACKAGE: &str = "Pop.Rpc";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const NAMESPACES: &[&str] = &["Pop.Rpc"];
pub const DEPENDENCIES: &[&str] = &[];
