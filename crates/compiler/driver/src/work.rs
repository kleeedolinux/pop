//! Private typed work records passed between front-end orchestration phases.
//!
//! These records make phase handoffs explicit without widening any Pop Lang
//! semantic contract or storing mutable compiler-global state.

use std::collections::{BTreeMap, BTreeSet};

use pop_compile_time::CompileTimeFunction;
use pop_foundation::{FunctionId, ModuleId, SourceSpan, SymbolId};
use pop_resolve::ResolutionDatabase;
use pop_source::SourceFile;
use pop_syntax::{AttributeUseSyntax, FunctionBodySyntax, SyntaxTree};
use pop_types::{
    AttributeTarget, BootstrapSchema, ClassDefinition, ClassMethodDefinition, ResolvedAttribute,
    ResolvedFunctionSignature,
};

pub(crate) struct ParsedModule {
    pub(crate) module: ModuleId,
    pub(crate) source: SourceFile,
    pub(crate) syntax: SyntaxTree,
}

pub(crate) struct FunctionWork {
    pub(crate) module: ModuleId,
    pub(crate) visibility: pop_resolve::Visibility,
    pub(crate) span: SourceSpan,
    pub(crate) body: FunctionBodySyntax,
    pub(crate) signature: ResolvedFunctionSignature,
    pub(crate) is_compile_time: bool,
    pub(crate) attribute_uses: Vec<AttributeUseSyntax>,
    pub(crate) attributes: Vec<ResolvedAttribute>,
}

pub(crate) struct CompileTimeContext {
    pub(crate) functions: BTreeMap<FunctionId, CompileTimeFunction>,
    pub(crate) eligible: BTreeSet<FunctionId>,
    pub(crate) names: BTreeMap<FunctionId, String>,
}

#[derive(Clone, Copy)]
pub(crate) struct AttributeResolutionContext<'context> {
    pub(crate) database: &'context ResolutionDatabase,
    pub(crate) bootstrap: &'context BootstrapSchema,
    pub(crate) signatures: &'context BTreeMap<SymbolId, ResolvedFunctionSignature>,
    pub(crate) compile_time: &'context CompileTimeContext,
}

pub(crate) struct DeclarationAttributeWork {
    pub(crate) module: ModuleId,
    pub(crate) symbol: SymbolId,
    pub(crate) target: AttributeTarget,
    pub(crate) attribute_uses: Vec<AttributeUseSyntax>,
    pub(crate) attributes: Vec<ResolvedAttribute>,
}

pub(crate) struct ConstantWork {
    pub(crate) module: ModuleId,
    pub(crate) symbol: SymbolId,
    pub(crate) syntax: pop_syntax::ConstDeclarationSyntax,
}

pub(crate) struct MethodWork {
    pub(crate) module: ModuleId,
    pub(crate) definition: ClassDefinition,
    pub(crate) method: ClassMethodDefinition,
    pub(crate) body: FunctionBodySyntax,
    pub(crate) signature: ResolvedFunctionSignature,
}
