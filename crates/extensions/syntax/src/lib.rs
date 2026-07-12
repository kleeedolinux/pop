//! Build metadata for the independently installed `Pop.Syntax` Package.

pub const PACKAGE: &str = "Pop.Syntax";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const NAMESPACES: &[&str] = &["Pop.Syntax", "Pop.Source"];
pub const DEPENDENCIES: &[&str] = &[];
