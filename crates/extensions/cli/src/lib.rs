//! Build metadata for the independently installed `Pop.Cli` Package.

pub const PACKAGE: &str = "Pop.Cli";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const NAMESPACES: &[&str] = &["Pop.Cli", "Pop.Command", "Pop.Settings"];
pub const DEPENDENCIES: &[&str] = &[];
