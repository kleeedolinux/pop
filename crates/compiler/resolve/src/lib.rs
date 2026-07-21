//! Declaration indexing, scopes, visibility, and static name resolution.
//!
//! `index` extracts declarations and `using` directives from lossless syntax,
//! `model` owns immutable indexed identities and visibility data, and `resolution`
//! performs deterministic typed-symbol lookup. Keep these phases separate: name
//! resolution must not become string-based runtime lookup or widen visibility.

mod index;
mod model;
mod resolution;

pub use index::{IndexResult, ModuleInput, build_declaration_index};
pub use model::{
    Declaration, DeclarationIndex, DeclarationKind, DeclarationOwner,
    GeneratedCodecSchemaDeclaration, GeneratedDeclarationError, ModuleIndex, ReferenceIndexError,
    ReferencedDeclaration, SymbolSpace, UsingDirective, Visibility,
};
pub use resolution::{PreludeNamespaceError, Resolution, ResolutionDatabase};
