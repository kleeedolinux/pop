//! Build metadata for the independently installed `Pop.Ai` Package.

pub const PACKAGE: &str = "Pop.Ai";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const NAMESPACES: &[&str] = &["Pop.Ai"];
pub const DEPENDENCIES: &[&str] = &[pop_extension_data::PACKAGE];
