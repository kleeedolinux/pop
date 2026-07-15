//! Build metadata for the independently installed `Pop.Ffi` Package.

pub const PACKAGE: &str = "Pop.Ffi";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const NAMESPACES: &[&str] = &["Ffi", "Ffi.C"];
pub const DEPENDENCIES: &[&str] = &[];
