//! Build metadata for the independently installed `Pop.Data` Package.

pub const PACKAGE: &str = "Pop.Data";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const NAMESPACES: &[&str] = &["Pop.Data", "Pop.Sql", "Pop.Store"];
pub const DEPENDENCIES: &[&str] = &[];
