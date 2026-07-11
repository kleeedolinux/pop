//! Declaration indexing, scopes, visibility, and static name resolution.

mod index;
mod model;
mod resolution;

pub use index::{IndexResult, ModuleInput, build_declaration_index};
pub use model::{
    Declaration, DeclarationIndex, DeclarationKind, DeclarationOwner, ModuleIndex, SymbolSpace,
    UsingDirective, Visibility,
};
pub use resolution::{Resolution, ResolutionDatabase};
