//! Typed build-time descriptors for Rust implementations of foundation APIs.
//!
//! `#[poplib]` does not create a Pop declaration. It exports one fixed native
//! symbol and describes the accepted Pop binding that must be verified by the
//! toolchain before linking.
//!
//! A Rust ABI mismatch is rejected at compile time:
//!
//! ```compile_fail
//! use pop_library_bridge::poplib;
//!
//! #[poplib(
//!     bubble = Standard,
//!     namespace = "Pop",
//!     name = "wrong",
//!     parameters(Int),
//!     results(),
//!     effects(),
//! )]
//! pub extern "C" fn wrong(value: u64) {
//!     let _ = value;
//! }
//! ```
//!
//! Unknown descriptor vocabulary is rejected by the attribute:
//!
//! ```compile_fail
//! use pop_library_bridge::poplib;
//!
//! #[poplib(
//!     bubble = Standard,
//!     namespace = "Pop",
//!     name = "dynamicValue",
//!     parameters(Dynamic),
//!     results(),
//!     effects(),
//! )]
//! pub extern "C" fn dynamic_value(_value: u64) {}
//! ```
//!
//! Adapter functions must be public, non-generic, and use the C ABI:
//!
//! ```compile_fail
//! use pop_library_bridge::poplib;
//!
//! #[poplib(
//!     bubble = Internal,
//!     namespace = "Pop.Internal.Text",
//!     name = "String.ByteAt",
//!     parameters(String, Int64),
//!     results(Byte),
//!     effects(),
//! )]
//! fn private_rust_function(_value: u64, _index: i64) -> u8 { 0 }
//! ```
//!
//! ```compile_fail
//! use pop_library_bridge::poplib;
//!
//! #[poplib(
//!     bubble = Standard,
//!     namespace = "Pop.Math",
//!     name = "identity",
//!     parameters(Int),
//!     results(Int),
//!     effects(),
//! )]
//! pub extern "C" fn generic_adapter<T>(_value: i64) -> i64 { 0 }
//! ```
//!
//! Duplicate descriptor fields are rejected rather than resolved by order:
//!
//! ```compile_fail
//! use pop_library_bridge::poplib;
//!
//! #[poplib(
//!     bubble = Standard,
//!     bubble = Internal,
//!     namespace = "Pop",
//!     name = "duplicate",
//!     parameters(),
//!     results(),
//!     effects(),
//! )]
//! pub extern "C" fn duplicate() {}
//! ```

pub use pop_library_macros::poplib;

/// The only Pop Lang Bubbles authorized to contain Rust foundation adapters.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum FoundationBubble {
    Standard,
    Internal,
}

/// Closed semantic types with an accepted bootstrap native ABI mapping.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum PopAbiType {
    Int,
    Int64,
    UInt64,
    Float,
    Boolean,
    Byte,
    String,
    ManagedReference,
}

/// Effects that a native adapter must declare to typed bootstrap metadata.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum NativeEffect {
    Allocates,
    WritesManagedReference,
    MayTrap,
    MayUnwind,
    Suspends,
    UnsafeMemory,
    ForeignFunction,
    AmbientIo,
    CompilerQuery,
    GcSafePoint,
    Roots,
}

/// Immutable compile-time description of one fixed native adapter.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NativeExport {
    bubble: FoundationBubble,
    namespace: &'static str,
    name: &'static str,
    native_symbol: &'static str,
    parameters: &'static [PopAbiType],
    results: &'static [PopAbiType],
    effects: &'static [NativeEffect],
}

impl NativeExport {
    #[doc(hidden)]
    #[must_use]
    pub const fn new(
        bubble: FoundationBubble,
        namespace: &'static str,
        name: &'static str,
        native_symbol: &'static str,
        parameters: &'static [PopAbiType],
        results: &'static [PopAbiType],
        effects: &'static [NativeEffect],
    ) -> Self {
        Self {
            bubble,
            namespace,
            name,
            native_symbol,
            parameters,
            results,
            effects,
        }
    }

    #[must_use]
    pub const fn bubble(self) -> FoundationBubble {
        self.bubble
    }

    #[must_use]
    pub const fn namespace(self) -> &'static str {
        self.namespace
    }

    #[must_use]
    pub const fn name(self) -> &'static str {
        self.name
    }

    #[must_use]
    pub const fn native_symbol(self) -> &'static str {
        self.native_symbol
    }

    #[must_use]
    pub const fn parameters(self) -> &'static [PopAbiType] {
        self.parameters
    }

    #[must_use]
    pub const fn results(self) -> &'static [PopAbiType] {
        self.results
    }

    #[must_use]
    pub const fn effects(self) -> &'static [NativeEffect] {
        self.effects
    }
}
