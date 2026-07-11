//! Build orchestration owned by the unified Pop Lang driver.

use std::collections::{BTreeMap, BTreeSet};

use pop_compile_time::{
    CompileTimeBudget, CompileTimeExpression, CompileTimeExpressionKind, CompileTimeFunction,
    CompileTimeInterpreter, CompileTimeLoweringError, CompileTimeProgram, CompileTimeValue,
    EvaluationError, EvaluationFailure, EvaluationFailureKind, EvaluationResult, ProgramError,
    lower_compile_time_expression, lower_compile_time_function,
};
use pop_diagnostics::compile_time as compile_time_diagnostics;
use pop_diagnostics::syntax as syntax_diagnostics;
use pop_foundation::{
    BubbleId, Diagnostic, DiagnosticOrigin, DiagnosticOriginKind, FunctionId, MethodId, ModuleId,
    NamespaceId, SourceSpan, SymbolId, TypeId,
};
use pop_hir::{
    HirBubble, HirDeclaration, HirDeclarationKind, HirFunction, HirFunctionContext,
    HirKnownCallables, HirMethod, build_hir_function_with_methods_and_attributes, build_hir_method,
};
use pop_query::{BudgetError, QueryBudget};
use pop_resolve::{ModuleInput, ResolutionDatabase, SymbolSpace, build_declaration_index};
use pop_source::SourceFile;
use pop_syntax::{
    AttributeUseSyntax, ExpressionSyntax, ExpressionSyntaxKind, FunctionBodySyntax, NodeKind,
    SyntaxTree, parse_attribute_declaration, parse_attribute_use, parse_class_declaration,
    parse_class_method_body, parse_file, parse_function_body, parse_function_signature,
    parse_record_declaration, parse_union_declaration,
};
use pop_types::{
    AttributeAttachmentError, AttributeConstant, AttributeQueryIndex, AttributeTarget,
    AttributeUsage, AttributeValidator, BodyChecker, BootstrapSchema, ClassDefinition,
    ClassMethodDefinition, CompilerAttributeRole, FieldDefault, PendingConstantExpression,
    ResolvedAttribute, ResolvedFunctionSignature, SignatureResolver, TypeArena, TypedBody,
    embedded_bootstrap_schema,
};

#[derive(Clone, Debug)]
pub struct FrontEndModule {
    module: ModuleId,
    source: SourceFile,
}

impl FrontEndModule {
    #[must_use]
    pub const fn new(module: ModuleId, source: SourceFile) -> Self {
        Self { module, source }
    }

    #[must_use]
    pub const fn module(&self) -> ModuleId {
        self.module
    }

    #[must_use]
    pub const fn source(&self) -> &SourceFile {
        &self.source
    }
}

#[derive(Clone, Debug)]
pub struct FrontEndBubbleInput {
    bubble: BubbleId,
    namespace: NamespaceId,
    dependencies: Vec<BubbleId>,
    modules: Vec<FrontEndModule>,
}

impl FrontEndBubbleInput {
    #[must_use]
    pub fn new(
        bubble: BubbleId,
        namespace: NamespaceId,
        mut dependencies: Vec<BubbleId>,
        mut modules: Vec<FrontEndModule>,
    ) -> Self {
        dependencies.sort_unstable();
        dependencies.dedup();
        modules.sort_by_key(FrontEndModule::module);
        Self {
            bubble,
            namespace,
            dependencies,
            modules,
        }
    }
}

#[derive(Clone, Debug)]
pub struct FrontEndResult {
    hir: Option<HirBubble>,
    types: TypeArena,
    attribute_queries: AttributeQueryIndex,
    compile_time_evaluations: Vec<FrontEndCompileTimeEvaluation>,
    diagnostics: Vec<Diagnostic>,
}

/// One source-requested compile-time outcome retained for incremental
/// dependency tracking and provenance-aware tooling.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FrontEndCompileTimeEvaluation {
    Result(EvaluationResult),
    Failure(EvaluationFailure),
}

impl FrontEndCompileTimeEvaluation {
    #[must_use]
    pub const fn result(&self) -> Option<&EvaluationResult> {
        match self {
            Self::Result(result) => Some(result),
            Self::Failure(_) => None,
        }
    }

    #[must_use]
    pub const fn failure(&self) -> Option<&EvaluationFailure> {
        match self {
            Self::Result(_) => None,
            Self::Failure(failure) => Some(failure),
        }
    }
}

impl FrontEndResult {
    #[must_use]
    pub const fn hir(&self) -> Option<&HirBubble> {
        self.hir.as_ref()
    }

    #[must_use]
    pub const fn types(&self) -> &TypeArena {
        &self.types
    }

    #[must_use]
    pub const fn attribute_queries(&self) -> &AttributeQueryIndex {
        &self.attribute_queries
    }

    #[must_use]
    pub fn compile_time_evaluations(&self) -> &[FrontEndCompileTimeEvaluation] {
        &self.compile_time_evaluations
    }

    #[must_use]
    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    #[must_use]
    pub fn diagnostic_snapshot(&self) -> String {
        diagnostic_snapshot(&self.diagnostics)
    }
}

struct ParsedModule {
    module: ModuleId,
    source: SourceFile,
    syntax: SyntaxTree,
}

struct FunctionWork {
    module: ModuleId,
    visibility: pop_resolve::Visibility,
    span: SourceSpan,
    body: FunctionBodySyntax,
    signature: ResolvedFunctionSignature,
    is_compile_time: bool,
    attribute_uses: Vec<AttributeUseSyntax>,
    attributes: Vec<ResolvedAttribute>,
}

struct CompileTimeContext {
    functions: BTreeMap<FunctionId, CompileTimeFunction>,
    eligible: BTreeSet<FunctionId>,
    names: BTreeMap<FunctionId, String>,
}

#[derive(Clone, Copy)]
struct AttributeResolutionContext<'context> {
    database: &'context ResolutionDatabase,
    bootstrap: &'context BootstrapSchema,
    signatures: &'context BTreeMap<SymbolId, ResolvedFunctionSignature>,
    compile_time: &'context CompileTimeContext,
}

struct DeclarationAttributeWork {
    module: ModuleId,
    symbol: SymbolId,
    target: AttributeTarget,
    attribute_uses: Vec<AttributeUseSyntax>,
    attributes: Vec<ResolvedAttribute>,
}

struct MethodWork {
    module: ModuleId,
    definition: ClassDefinition,
    method: ClassMethodDefinition,
    body: FunctionBodySyntax,
    signature: ResolvedFunctionSignature,
}

/// Runs the architecture-ordered front end through verified backend-neutral HIR.
///
/// # Panics
///
/// Panics only if repository-validated bootstrap metadata is invalid or if a
/// resolver symbol cannot be published once in the immutable attribute-query
/// snapshot. Both are toolchain incidents guarded by bootstrap/query tests.
#[must_use]
pub fn analyze_bubble(input: FrontEndBubbleInput) -> FrontEndResult {
    let parsed = parse_modules(input.modules);
    let module_inputs: Vec<_> = parsed
        .iter()
        .map(|module| ModuleInput::new(module.module, input.bubble, &module.source, &module.syntax))
        .collect();
    let indexed = build_declaration_index(&module_inputs);
    let mut diagnostics = indexed.diagnostics().to_vec();
    validate_source_attribute_targets(&parsed, &mut diagnostics);
    let database = ResolutionDatabase::new(indexed.into_index());
    let bootstrap = embedded_bootstrap_schema().expect("repository-validated bootstrap schema");
    let mut resolver = SignatureResolver::new(&database, bootstrap.clone());
    let (mut declarations, methods, mut declaration_attributes) = define_declarations(
        &parsed,
        input.bubble,
        &database,
        &mut resolver,
        &mut diagnostics,
    );
    let mut functions = resolve_functions(
        &parsed,
        &database,
        &bootstrap,
        &mut resolver,
        &mut diagnostics,
    );
    let signatures: BTreeMap<_, _> = functions
        .iter()
        .map(|function| (function.signature.symbol(), function.signature.clone()))
        .collect();
    let preliminary_bodies = check_compile_time_function_bodies(
        &functions,
        &signatures,
        &mut resolver,
        &mut diagnostics,
    );
    let compile_time = build_compile_time_context(
        &functions,
        &preliminary_bodies,
        resolver.arena(),
        &mut diagnostics,
    );
    let mut compile_time_evaluations = Vec::new();
    evaluate_declaration_defaults(
        &declarations,
        &signatures,
        &compile_time,
        &mut resolver,
        &mut compile_time_evaluations,
        &mut diagnostics,
    );
    let attribute_queries = resolve_source_attributes(
        &mut declaration_attributes,
        &mut functions,
        AttributeResolutionContext {
            database: &database,
            bootstrap: &bootstrap,
            signatures: &signatures,
            compile_time: &compile_time,
        },
        &mut resolver,
        &mut compile_time_evaluations,
        &mut diagnostics,
    );
    declarations = refresh_declarations(declarations, &resolver);
    let (hir_functions, hir_methods) = build_runtime_hir(
        input.bubble,
        &mut functions,
        &methods,
        &signatures,
        &mut resolver,
        &mut diagnostics,
    );
    sort_diagnostics(&mut diagnostics);
    let hir = if diagnostics.is_empty() {
        HirBubble::new_with_declarations_and_methods(
            input.bubble,
            input.namespace,
            input.dependencies,
            declarations,
            hir_functions,
            hir_methods,
        )
        .ok()
    } else {
        None
    };
    FrontEndResult {
        hir,
        types: resolver.into_arena(),
        attribute_queries,
        compile_time_evaluations,
        diagnostics,
    }
}

fn parse_modules(modules: Vec<FrontEndModule>) -> Vec<ParsedModule> {
    modules
        .into_iter()
        .map(|module| ParsedModule {
            module: module.module,
            syntax: parse_file(&module.source),
            source: module.source,
        })
        .collect()
}

fn validate_source_attribute_targets(modules: &[ParsedModule], diagnostics: &mut Vec<Diagnostic>) {
    for module in modules {
        let children = module.syntax.root().children();
        let mut index = 0_usize;
        while index < children.len() {
            if children[index].kind() != NodeKind::AttributeUse {
                index += 1;
                continue;
            }
            let first = index;
            while index < children.len() && children[index].kind() == NodeKind::AttributeUse {
                index += 1;
            }
            let supported = children.get(index).is_some_and(|node| {
                matches!(
                    node.kind(),
                    NodeKind::AttributeDeclaration
                        | NodeKind::RecordDeclaration
                        | NodeKind::UnionDeclaration
                        | NodeKind::ClassDeclaration
                        | NodeKind::FunctionDeclaration
                )
            });
            if supported {
                continue;
            }
            for attribute in &children[first..index] {
                diagnostics.push(compile_time_diagnostics::ineligible_constant_expression(
                    SourceSpan::new(module.source.id(), attribute.range()),
                    "attribute attachment target",
                ));
            }
        }
    }
}

fn build_runtime_hir(
    bubble: BubbleId,
    functions: &mut [FunctionWork],
    methods: &[MethodWork],
    signatures: &BTreeMap<SymbolId, ResolvedFunctionSignature>,
    resolver: &mut SignatureResolver<'_>,
    diagnostics: &mut Vec<Diagnostic>,
) -> (Vec<HirFunction>, Vec<HirMethod>) {
    let known_functions: BTreeSet<_> = functions
        .iter()
        .filter(|function| !function.is_compile_time)
        .map(|function| function.signature.symbol())
        .collect();
    let known_methods: BTreeSet<MethodId> =
        methods.iter().map(|work| work.method.method()).collect();
    let mut hir_functions = Vec::new();
    for function in functions {
        if function.is_compile_time {
            continue;
        }
        let typed = BodyChecker::new(function.module, resolver, signatures)
            .check(&function.signature, &function.body);
        diagnostics.extend(typed.diagnostics().iter().cloned());
        let Some(body) = typed.body() else {
            continue;
        };
        if let Ok(function) = build_hir_function_with_methods_and_attributes(
            HirFunctionContext::new(function.module, bubble, function.visibility),
            &function.signature,
            body,
            resolver.arena(),
            &known_functions,
            &known_methods,
            &function.attributes,
        ) {
            hir_functions.push(function);
        }
    }
    let mut hir_methods = Vec::new();
    for method in methods {
        let typed = BodyChecker::new(method.module, resolver, signatures)
            .check(&method.signature, &method.body);
        diagnostics.extend(typed.diagnostics().iter().cloned());
        let Some(body) = typed.body() else {
            continue;
        };
        let definition = resolver
            .class_definition(method.definition.symbol())
            .unwrap_or(&method.definition);
        if let Ok(lowered) = build_hir_method(
            HirFunctionContext::new(method.module, bubble, method.method.visibility()),
            definition,
            &method.method,
            &method.signature,
            body,
            resolver.arena(),
            HirKnownCallables::new(&known_functions, &known_methods),
        ) {
            hir_methods.push(lowered);
        }
    }
    (hir_functions, hir_methods)
}

fn define_declarations(
    modules: &[ParsedModule],
    bubble: BubbleId,
    database: &ResolutionDatabase,
    resolver: &mut SignatureResolver<'_>,
    diagnostics: &mut Vec<Diagnostic>,
) -> (
    Vec<HirDeclaration>,
    Vec<MethodWork>,
    Vec<DeclarationAttributeWork>,
) {
    let (mut declarations, mut attribute_work) =
        define_attributes(modules, bubble, database, resolver, diagnostics);
    let (methods, mut data_declarations, mut data_attribute_work) =
        define_data(modules, bubble, database, resolver, diagnostics);
    declarations.append(&mut data_declarations);
    attribute_work.append(&mut data_attribute_work);
    declarations.sort_by_key(HirDeclaration::symbol);
    attribute_work.sort_by_key(|work| work.symbol);
    (declarations, methods, attribute_work)
}

fn define_attributes(
    modules: &[ParsedModule],
    bubble: BubbleId,
    database: &ResolutionDatabase,
    resolver: &mut SignatureResolver<'_>,
    diagnostics: &mut Vec<Diagnostic>,
) -> (Vec<HirDeclaration>, Vec<DeclarationAttributeWork>) {
    let mut declarations = Vec::new();
    let mut attribute_work = Vec::new();
    for module in modules {
        let mut pending_attributes = Vec::new();
        for node in module.syntax.root().children() {
            if node.kind() == NodeKind::AttributeUse {
                match parse_attribute_use(&module.source, &module.syntax, node) {
                    Ok(syntax) => pending_attributes.push(syntax),
                    Err(error) => diagnostics.push(syntax_error(error.span(), error.expectation())),
                }
                continue;
            }
            if node.kind() != NodeKind::AttributeDeclaration {
                pending_attributes.clear();
                continue;
            }
            let attribute_uses = std::mem::take(&mut pending_attributes);
            match parse_attribute_declaration(&module.source, &module.syntax, node) {
                Ok(syntax) => {
                    if let Some(symbol) = resolve_symbol(
                        database,
                        module.module,
                        syntax.name(),
                        SymbolSpace::Type,
                        syntax.span(),
                        diagnostics,
                    ) {
                        let result =
                            resolver.define_attribute_schema(module.module, symbol, &syntax);
                        diagnostics.extend(result.diagnostics().iter().cloned());
                        if let (Some(definition), Some(declaration)) =
                            (result.definition(), database.index().declaration(symbol))
                        {
                            attribute_work.push(DeclarationAttributeWork {
                                module: module.module,
                                symbol,
                                target: AttributeTarget::Attribute,
                                attribute_uses,
                                attributes: Vec::new(),
                            });
                            declarations.push(HirDeclaration::attribute(
                                module.module,
                                bubble,
                                declaration.visibility(),
                                syntax.name(),
                                definition,
                            ));
                        }
                    }
                }
                Err(error) => {
                    diagnostics.push(syntax_error(error.span(), error.expectation()));
                }
            }
        }
    }
    (declarations, attribute_work)
}

fn define_data(
    modules: &[ParsedModule],
    bubble: BubbleId,
    database: &ResolutionDatabase,
    resolver: &mut SignatureResolver<'_>,
    diagnostics: &mut Vec<Diagnostic>,
) -> (
    Vec<MethodWork>,
    Vec<HirDeclaration>,
    Vec<DeclarationAttributeWork>,
) {
    let mut methods = Vec::new();
    let mut declarations = Vec::new();
    let mut attribute_work = Vec::new();
    for module in modules {
        let mut pending_attributes = Vec::new();
        for node in module.syntax.root().children() {
            if node.kind() == NodeKind::AttributeUse {
                match parse_attribute_use(&module.source, &module.syntax, node) {
                    Ok(syntax) => pending_attributes.push(syntax),
                    Err(error) => diagnostics.push(syntax_error(error.span(), error.expectation())),
                }
                continue;
            }
            match node.kind() {
                NodeKind::RecordDeclaration => {
                    let (declaration, work) = define_record(
                        module,
                        node,
                        bubble,
                        database,
                        resolver,
                        diagnostics,
                        std::mem::take(&mut pending_attributes),
                    );
                    declarations.extend(declaration);
                    attribute_work.extend(work);
                }
                NodeKind::UnionDeclaration => {
                    let (declaration, work) = define_union(
                        module,
                        node,
                        bubble,
                        database,
                        resolver,
                        diagnostics,
                        std::mem::take(&mut pending_attributes),
                    );
                    declarations.extend(declaration);
                    attribute_work.extend(work);
                }
                NodeKind::ClassDeclaration => {
                    let (class_methods, declaration, work) = define_class(
                        module,
                        node,
                        bubble,
                        database,
                        resolver,
                        diagnostics,
                        std::mem::take(&mut pending_attributes),
                    );
                    methods.extend(class_methods);
                    declarations.extend(declaration);
                    attribute_work.extend(work);
                }
                _ => pending_attributes.clear(),
            }
        }
    }
    methods.sort_by_key(|method| method.method.method());
    declarations.sort_by_key(HirDeclaration::symbol);
    attribute_work.sort_by_key(|work| work.symbol);
    (methods, declarations, attribute_work)
}

fn define_record(
    module: &ParsedModule,
    node: &pop_syntax::SyntaxNode,
    bubble: BubbleId,
    database: &ResolutionDatabase,
    resolver: &mut SignatureResolver<'_>,
    diagnostics: &mut Vec<Diagnostic>,
    attribute_uses: Vec<AttributeUseSyntax>,
) -> (Option<HirDeclaration>, Option<DeclarationAttributeWork>) {
    let syntax = match parse_record_declaration(&module.source, &module.syntax, node) {
        Ok(syntax) => syntax,
        Err(error) => {
            diagnostics.push(syntax_error(error.span(), error.expectation()));
            return (None, None);
        }
    };
    let Some(symbol) = resolve_symbol(
        database,
        module.module,
        syntax.name(),
        SymbolSpace::Type,
        syntax.span(),
        diagnostics,
    ) else {
        return (None, None);
    };
    let result = resolver.define_record_schema(module.module, symbol, &syntax);
    diagnostics.extend(result.diagnostics().iter().cloned());
    let (Some(definition), Some(declaration)) =
        (result.definition(), database.index().declaration(symbol))
    else {
        return (None, None);
    };
    (
        Some(HirDeclaration::record(
            module.module,
            bubble,
            declaration.visibility(),
            syntax.name(),
            definition,
        )),
        Some(DeclarationAttributeWork {
            module: module.module,
            symbol,
            target: AttributeTarget::Record,
            attribute_uses,
            attributes: Vec::new(),
        }),
    )
}

fn define_union(
    module: &ParsedModule,
    node: &pop_syntax::SyntaxNode,
    bubble: BubbleId,
    database: &ResolutionDatabase,
    resolver: &mut SignatureResolver<'_>,
    diagnostics: &mut Vec<Diagnostic>,
    attribute_uses: Vec<AttributeUseSyntax>,
) -> (Option<HirDeclaration>, Option<DeclarationAttributeWork>) {
    let syntax = match parse_union_declaration(&module.source, &module.syntax, node) {
        Ok(syntax) => syntax,
        Err(error) => {
            diagnostics.push(syntax_error(error.span(), error.expectation()));
            return (None, None);
        }
    };
    let Some(symbol) = resolve_symbol(
        database,
        module.module,
        syntax.name(),
        SymbolSpace::Type,
        syntax.span(),
        diagnostics,
    ) else {
        return (None, None);
    };
    let result = resolver.define_union(module.module, symbol, &syntax);
    diagnostics.extend(result.diagnostics().iter().cloned());
    let (Some(definition), Some(declaration)) =
        (result.definition(), database.index().declaration(symbol))
    else {
        return (None, None);
    };
    (
        Some(HirDeclaration::tagged_union(
            module.module,
            bubble,
            declaration.visibility(),
            syntax.name(),
            definition,
        )),
        Some(DeclarationAttributeWork {
            module: module.module,
            symbol,
            target: AttributeTarget::Union,
            attribute_uses,
            attributes: Vec::new(),
        }),
    )
}

fn define_class(
    module: &ParsedModule,
    node: &pop_syntax::SyntaxNode,
    bubble: BubbleId,
    database: &ResolutionDatabase,
    resolver: &mut SignatureResolver<'_>,
    diagnostics: &mut Vec<Diagnostic>,
    attribute_uses: Vec<AttributeUseSyntax>,
) -> (
    Vec<MethodWork>,
    Option<HirDeclaration>,
    Option<DeclarationAttributeWork>,
) {
    let syntax = match parse_class_declaration(&module.source, &module.syntax, node) {
        Ok(syntax) => syntax,
        Err(error) => {
            diagnostics.push(syntax_error(error.span(), error.expectation()));
            return (Vec::new(), None, None);
        }
    };
    let Some(symbol) = resolve_symbol(
        database,
        module.module,
        syntax.name(),
        SymbolSpace::Type,
        syntax.span(),
        diagnostics,
    ) else {
        return (Vec::new(), None, None);
    };
    let result = resolver.define_class_schema(module.module, symbol, &syntax);
    diagnostics.extend(result.diagnostics().iter().cloned());
    let Some(definition) = result.definition().cloned() else {
        return (Vec::new(), None, None);
    };
    let declaration = database.index().declaration(symbol).map(|declaration| {
        HirDeclaration::class(
            module.module,
            bubble,
            declaration.visibility(),
            syntax.name(),
            &definition,
        )
    });
    let methods = syntax
        .methods()
        .iter()
        .zip(definition.methods())
        .filter_map(|(method_syntax, method)| {
            match parse_class_method_body(&module.source, &module.syntax, node, method_syntax) {
                Ok(body) => Some(MethodWork {
                    module: module.module,
                    signature: resolver.method_signature(&definition, method),
                    definition: definition.clone(),
                    method: method.clone(),
                    body,
                }),
                Err(error) => {
                    diagnostics.push(syntax_error(error.span(), error.expectation()));
                    None
                }
            }
        })
        .collect();
    let attribute_work = Some(DeclarationAttributeWork {
        module: module.module,
        symbol,
        target: AttributeTarget::Class,
        attribute_uses,
        attributes: Vec::new(),
    });
    (methods, declaration, attribute_work)
}

fn resolve_functions(
    modules: &[ParsedModule],
    database: &ResolutionDatabase,
    bootstrap: &BootstrapSchema,
    resolver: &mut SignatureResolver<'_>,
    diagnostics: &mut Vec<Diagnostic>,
) -> Vec<FunctionWork> {
    let mut functions = Vec::new();
    for module in modules {
        let mut pending_attributes = Vec::new();
        for node in module.syntax.root().children() {
            if node.kind() == NodeKind::AttributeUse {
                match parse_attribute_use(&module.source, &module.syntax, node) {
                    Ok(syntax) => pending_attributes.push(syntax),
                    Err(error) => {
                        diagnostics.push(syntax_error(error.span(), error.expectation()));
                    }
                }
                continue;
            }
            if node.kind() != NodeKind::FunctionDeclaration {
                pending_attributes.clear();
                continue;
            }
            if let Some(function) = resolve_function(
                module,
                node,
                database,
                bootstrap,
                resolver,
                diagnostics,
                std::mem::take(&mut pending_attributes),
            ) {
                functions.push(function);
            }
        }
    }
    functions.sort_by_key(|function| function.signature.symbol());
    functions
}

fn resolve_function(
    module: &ParsedModule,
    node: &pop_syntax::SyntaxNode,
    database: &ResolutionDatabase,
    bootstrap: &BootstrapSchema,
    resolver: &mut SignatureResolver<'_>,
    diagnostics: &mut Vec<Diagnostic>,
    attribute_uses: Vec<AttributeUseSyntax>,
) -> Option<FunctionWork> {
    let syntax_signature = parse_function_signature(&module.source, &module.syntax, node)
        .map_err(|error| syntax_error(error.span(), error.expectation()))
        .map_err(|diagnostic| diagnostics.push(diagnostic))
        .ok()?;
    let body = parse_function_body(&module.source, &module.syntax, node, &syntax_signature)
        .map_err(|error| syntax_error(error.span(), error.expectation()))
        .map_err(|diagnostic| diagnostics.push(diagnostic))
        .ok()?;
    let span = SourceSpan::new(module.source.id(), syntax_signature.range());
    let symbol = resolve_symbol(
        database,
        module.module,
        syntax_signature.name(),
        SymbolSpace::Value,
        span,
        diagnostics,
    )?;
    let declaration = database.index().declaration(symbol)?;
    let result = resolver.resolve(module.module, symbol, &syntax_signature);
    diagnostics.extend(result.diagnostics().iter().cloned());
    let (is_compile_time, attribute_uses) =
        classify_function_attributes(database, bootstrap, module.module, attribute_uses);
    Some(FunctionWork {
        module: module.module,
        visibility: declaration.visibility(),
        span,
        body,
        signature: result.signature()?.clone(),
        is_compile_time,
        attribute_uses,
        attributes: Vec::new(),
    })
}

fn classify_function_attributes(
    database: &ResolutionDatabase,
    bootstrap: &BootstrapSchema,
    module: ModuleId,
    attributes: Vec<AttributeUseSyntax>,
) -> (bool, Vec<AttributeUseSyntax>) {
    let mut marked = false;
    let mut ordinary = Vec::new();
    for attribute in attributes {
        if trusted_compiler_attribute_role(database, bootstrap, module, &attribute)
            == Some(CompilerAttributeRole::CompileTime)
            && attribute.arguments().is_empty()
        {
            marked = true;
        } else {
            ordinary.push(attribute);
        }
    }
    (marked, ordinary)
}

fn trusted_compiler_attribute_role(
    database: &ResolutionDatabase,
    bootstrap: &BootstrapSchema,
    module: ModuleId,
    attribute: &AttributeUseSyntax,
) -> Option<CompilerAttributeRole> {
    let [name] = attribute.path() else {
        return None;
    };
    let entry = bootstrap.compiler_attribute_by_source_name(name)?;
    let shadowed = database
        .resolve(module, name, SymbolSpace::Type, attribute.span())
        .symbol()
        .is_some();
    (!shadowed).then(|| entry.role())
}

fn resolve_attribute_contracts(
    work: &mut [DeclarationAttributeWork],
    database: &ResolutionDatabase,
    bootstrap: &BootstrapSchema,
    compile_time: &CompileTimeContext,
    resolver: &mut SignatureResolver<'_>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    for declaration in work {
        if declaration.target != AttributeTarget::Attribute {
            continue;
        }
        let mut ordinary = Vec::new();
        for syntax in std::mem::take(&mut declaration.attribute_uses) {
            match trusted_compiler_attribute_role(database, bootstrap, declaration.module, &syntax)
            {
                Some(CompilerAttributeRole::AttributeUsage) => {
                    match parse_attribute_usage_contract(database, declaration.module, &syntax) {
                        Some(usage) => {
                            if resolver
                                .install_attribute_usage(declaration.symbol, usage)
                                .is_err()
                            {
                                diagnostics.push(
                                    compile_time_diagnostics::ineligible_constant_expression(
                                        syntax.span(),
                                        "duplicate AttributeUsage contract",
                                    ),
                                );
                            }
                        }
                        None => diagnostics.push(
                            compile_time_diagnostics::ineligible_constant_expression(
                                syntax.span(),
                                "AttributeUsage contract",
                            ),
                        ),
                    }
                }
                Some(CompilerAttributeRole::AttributeValidator) => {
                    match resolve_attribute_validator(
                        database,
                        declaration.module,
                        &syntax,
                        compile_time,
                    ) {
                        Some(validator) => {
                            if resolver
                                .install_attribute_validator(declaration.symbol, validator)
                                .is_err()
                            {
                                diagnostics.push(
                                    compile_time_diagnostics::ineligible_constant_expression(
                                        syntax.span(),
                                        "duplicate AttributeValidator contract",
                                    ),
                                );
                            }
                        }
                        None => diagnostics.push(
                            compile_time_diagnostics::ineligible_constant_expression(
                                syntax.span(),
                                "AttributeValidator function",
                            ),
                        ),
                    }
                }
                _ => ordinary.push(syntax),
            }
        }
        declaration.attribute_uses = ordinary;
    }
}

fn parse_attribute_usage_contract(
    database: &ResolutionDatabase,
    module: ModuleId,
    syntax: &AttributeUseSyntax,
) -> Option<AttributeUsage> {
    if syntax.arguments().len() != 2 {
        return None;
    }
    let mut targets = None;
    let mut repeatable = None;
    let mut next_positional = 0_usize;
    for argument in syntax.arguments() {
        let index = match argument.name() {
            Some("targets") => 0,
            Some("repeatable") => 1,
            Some(_) => return None,
            None => {
                let index = next_positional;
                next_positional = next_positional.saturating_add(1);
                index
            }
        };
        match index {
            0 if targets.is_none() => {
                targets = parse_attribute_targets(database, module, argument.value());
                targets.as_ref()?;
            }
            1 if repeatable.is_none() => {
                let ExpressionSyntaxKind::Boolean(value) = argument.value().kind() else {
                    return None;
                };
                repeatable = Some(*value);
            }
            _ => return None,
        }
    }
    Some(AttributeUsage::new(targets?, repeatable?))
}

fn parse_attribute_targets(
    database: &ResolutionDatabase,
    module: ModuleId,
    expression: &ExpressionSyntax,
) -> Option<Vec<AttributeTarget>> {
    if database
        .resolve(
            module,
            "AttributeTarget",
            SymbolSpace::Type,
            expression.span(),
        )
        .symbol()
        .is_some()
    {
        return None;
    }
    let ExpressionSyntaxKind::Array(elements) = expression.kind() else {
        return None;
    };
    elements
        .iter()
        .map(|element| {
            let ExpressionSyntaxKind::Name(path) = element.kind() else {
                return None;
            };
            let [owner, case] = path.as_slice() else {
                return None;
            };
            if owner != "AttributeTarget" {
                return None;
            }
            match case.as_str() {
                "Namespace" => Some(AttributeTarget::Namespace),
                "Function" => Some(AttributeTarget::Function),
                "Constant" => Some(AttributeTarget::Constant),
                "TypeAlias" => Some(AttributeTarget::TypeAlias),
                "Attribute" => Some(AttributeTarget::Attribute),
                "Record" => Some(AttributeTarget::Record),
                "Union" => Some(AttributeTarget::Union),
                "Class" => Some(AttributeTarget::Class),
                "Interface" => Some(AttributeTarget::Interface),
                "Enum" => Some(AttributeTarget::Enum),
                "Field" => Some(AttributeTarget::Field),
                "Case" => Some(AttributeTarget::Case),
                "Method" => Some(AttributeTarget::Method),
                _ => None,
            }
        })
        .collect()
}

fn resolve_attribute_validator(
    database: &ResolutionDatabase,
    module: ModuleId,
    syntax: &AttributeUseSyntax,
    compile_time: &CompileTimeContext,
) -> Option<AttributeValidator> {
    let [argument] = syntax.arguments() else {
        return None;
    };
    if argument.name().is_some() {
        return None;
    }
    let ExpressionSyntaxKind::Name(path) = argument.value().kind() else {
        return None;
    };
    let name = path.join(".");
    let symbol = database
        .resolve(module, &name, SymbolSpace::Value, argument.value().span())
        .symbol()?;
    let function = FunctionId::from_raw(symbol.raw());
    compile_time
        .eligible
        .contains(&function)
        .then(|| AttributeValidator::new(function))
}

fn resolve_source_attributes(
    declarations: &mut [DeclarationAttributeWork],
    functions: &mut [FunctionWork],
    context: AttributeResolutionContext<'_>,
    resolver: &mut SignatureResolver<'_>,
    compile_time_evaluations: &mut Vec<FrontEndCompileTimeEvaluation>,
    diagnostics: &mut Vec<Diagnostic>,
) -> AttributeQueryIndex {
    resolve_attribute_contracts(
        declarations,
        context.database,
        context.bootstrap,
        context.compile_time,
        resolver,
        diagnostics,
    );
    resolve_declaration_attributes(
        declarations,
        context.database,
        context.signatures,
        context.compile_time,
        resolver,
        compile_time_evaluations,
        diagnostics,
    );
    resolve_function_attributes(
        functions,
        context.database,
        context.signatures,
        context.compile_time,
        resolver,
        compile_time_evaluations,
        diagnostics,
    );
    build_attribute_query_index(declarations, functions, resolver)
}

fn resolve_function_attributes(
    functions: &mut [FunctionWork],
    database: &ResolutionDatabase,
    signatures: &BTreeMap<SymbolId, ResolvedFunctionSignature>,
    compile_time: &CompileTimeContext,
    resolver: &mut SignatureResolver<'_>,
    compile_time_evaluations: &mut Vec<FrontEndCompileTimeEvaluation>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    for function in functions {
        let resolved_attributes = resolve_attribute_uses(
            function.module,
            AttributeTarget::Function,
            &function.attribute_uses,
            database,
            signatures,
            compile_time,
            resolver,
            compile_time_evaluations,
            diagnostics,
        );
        let validated =
            resolver.validate_attribute_attachments(AttributeTarget::Function, resolved_attributes);
        diagnostics.extend(
            validated
                .errors()
                .iter()
                .map(attribute_attachment_diagnostic),
        );
        if let Some(attachments) = validated.attachment_set() {
            function.attributes = attachments.attachments().to_vec();
        }
    }
}

fn resolve_declaration_attributes(
    declarations: &mut [DeclarationAttributeWork],
    database: &ResolutionDatabase,
    signatures: &BTreeMap<SymbolId, ResolvedFunctionSignature>,
    compile_time: &CompileTimeContext,
    resolver: &mut SignatureResolver<'_>,
    compile_time_evaluations: &mut Vec<FrontEndCompileTimeEvaluation>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    for declaration in declarations {
        let resolved_attributes = resolve_attribute_uses(
            declaration.module,
            declaration.target,
            &declaration.attribute_uses,
            database,
            signatures,
            compile_time,
            resolver,
            compile_time_evaluations,
            diagnostics,
        );
        let validated =
            resolver.validate_attribute_attachments(declaration.target, resolved_attributes);
        diagnostics.extend(
            validated
                .errors()
                .iter()
                .map(attribute_attachment_diagnostic),
        );
        if let Some(attachments) = validated.attachment_set() {
            declaration.attributes = attachments.attachments().to_vec();
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn resolve_attribute_uses(
    module: ModuleId,
    target: AttributeTarget,
    attribute_uses: &[AttributeUseSyntax],
    database: &ResolutionDatabase,
    signatures: &BTreeMap<SymbolId, ResolvedFunctionSignature>,
    compile_time: &CompileTimeContext,
    resolver: &mut SignatureResolver<'_>,
    compile_time_evaluations: &mut Vec<FrontEndCompileTimeEvaluation>,
    diagnostics: &mut Vec<Diagnostic>,
) -> Vec<ResolvedAttribute> {
    let mut attributes = Vec::new();
    for syntax in attribute_uses {
        let name = syntax.path().join(".");
        let definition = database
            .resolve(module, &name, SymbolSpace::Type, syntax.span())
            .symbol()
            .and_then(|symbol| resolver.attribute_definition(symbol))
            .cloned();
        let Some(definition) = definition else {
            let result = resolver.resolve_attribute_use(module, syntax);
            diagnostics.extend(result.diagnostics().iter().cloned());
            continue;
        };
        if !definition.usage().allows(target) {
            diagnostics.push(attribute_attachment_diagnostic(
                &AttributeAttachmentError::WrongTarget {
                    attribute: definition.attribute(),
                    target,
                    span: syntax.span(),
                },
            ));
            continue;
        }
        let mut evaluated = Vec::new();
        let mut next_positional = 0;
        for argument in syntax.arguments() {
            let index = if let Some(name) = argument.name() {
                definition
                    .parameters()
                    .iter()
                    .position(|parameter| parameter.name() == name)
            } else {
                let index = next_positional;
                next_positional += 1;
                Some(index)
            };
            let Some(expected) = index
                .and_then(|index| definition.parameters().get(index))
                .map(pop_types::AttributeParameterDefinition::parameter_type)
            else {
                continue;
            };
            let value = evaluate_required_expression(
                module,
                argument.value(),
                expected,
                signatures,
                compile_time,
                resolver,
                compile_time_evaluations,
            )
            .and_then(|value| {
                compile_time_attribute_constant(value).ok_or_else(|| {
                    vec![compile_time_diagnostics::ineligible_constant_expression(
                        argument.value().span(),
                        "attribute argument",
                    )]
                })
            });
            evaluated.push((argument.value().span(), expected, value));
        }
        let result = resolver.resolve_attribute_use_with_evaluator(
            module,
            syntax,
            |expression, expected| {
                evaluated
                    .iter()
                    .find(|(span, cached_expected, _)| {
                        *span == expression.span() && *cached_expected == expected
                    })
                    .map_or_else(
                        || {
                            Err(vec![
                                compile_time_diagnostics::ineligible_constant_expression(
                                    expression.span(),
                                    "attribute argument",
                                ),
                            ])
                        },
                        |(_, _, value)| value.clone(),
                    )
            },
        );
        diagnostics.extend(result.diagnostics().iter().cloned());
        if let Some(attribute) = result.attribute() {
            attributes.push(attribute.clone());
        }
    }
    attributes
}

fn attribute_attachment_diagnostic(error: &AttributeAttachmentError) -> Diagnostic {
    let (span, context) = match error {
        AttributeAttachmentError::UnknownAttribute { attribute, span } => {
            (*span, format!("unknown attribute#{}", attribute.raw()))
        }
        AttributeAttachmentError::WrongTarget {
            attribute,
            target,
            span,
        } => (
            *span,
            format!("attribute#{} cannot target {target:?}", attribute.raw()),
        ),
        AttributeAttachmentError::NonRepeatableDuplicate {
            attribute,
            duplicate,
            ..
        } => (
            *duplicate,
            format!("attribute#{} is not repeatable", attribute.raw()),
        ),
    };
    compile_time_diagnostics::ineligible_constant_expression(span, context)
}

fn build_attribute_query_index(
    declarations: &[DeclarationAttributeWork],
    functions: &[FunctionWork],
    resolver: &SignatureResolver<'_>,
) -> AttributeQueryIndex {
    let mut index = resolver.attribute_query_index();
    for declaration in declarations {
        let validated = resolver.validate_attribute_attachments(
            declaration.target,
            declaration.attributes.iter().cloned(),
        );
        if let Some(attachments) = validated.attachment_set() {
            index
                .insert_symbol(declaration.symbol, attachments.clone())
                .expect("validated declaration has one indexed resolver symbol");
        }
    }
    for function in functions {
        let validated = resolver.validate_attribute_attachments(
            AttributeTarget::Function,
            function.attributes.iter().cloned(),
        );
        if let Some(attachments) = validated.attachment_set() {
            index
                .insert_symbol(function.signature.symbol(), attachments.clone())
                .expect("validated function has one indexed resolver symbol");
        }
    }
    index
}

fn check_compile_time_function_bodies(
    functions: &[FunctionWork],
    signatures: &BTreeMap<SymbolId, ResolvedFunctionSignature>,
    resolver: &mut SignatureResolver<'_>,
    diagnostics: &mut Vec<Diagnostic>,
) -> BTreeMap<SymbolId, TypedBody> {
    let mut bodies = BTreeMap::new();
    for function in functions {
        let typed = BodyChecker::new(function.module, resolver, signatures)
            .check(&function.signature, &function.body);
        if function.is_compile_time {
            diagnostics.extend(typed.diagnostics().iter().cloned());
        }
        if let Some(body) = typed.body() {
            bodies.insert(function.signature.symbol(), body.clone());
        }
    }
    bodies
}

fn build_compile_time_context(
    functions: &[FunctionWork],
    bodies: &BTreeMap<SymbolId, TypedBody>,
    types: &TypeArena,
    diagnostics: &mut Vec<Diagnostic>,
) -> CompileTimeContext {
    let mut lowered = BTreeMap::new();
    let mut eligible = BTreeSet::new();
    let mut names = BTreeMap::new();
    for function in functions {
        let id = FunctionId::from_raw(function.signature.symbol().raw());
        names.insert(id, function.signature.name().to_owned());
        let Some(body) = bodies.get(&function.signature.symbol()) else {
            continue;
        };
        match lower_compile_time_function(&function.signature, body, types) {
            Ok(definition) => {
                if function.is_compile_time {
                    eligible.insert(id);
                }
                lowered.insert(id, definition);
            }
            Err(error) if function.is_compile_time => {
                diagnostics.push(compile_time_diagnostics::forbidden_effect(
                    compile_time_lowering_span(error, function.span),
                    function.signature.name(),
                    format!("{error:?}"),
                    [],
                ));
            }
            Err(_) => {}
        }
    }
    for function in &eligible {
        let Some(definition) = lowered.get(function) else {
            continue;
        };
        for (called, span) in direct_calls(definition.body()) {
            if !eligible.contains(&called) {
                diagnostics.push(compile_time_diagnostics::function_not_eligible(
                    span,
                    compile_time_function_name(&names, called),
                    [],
                ));
            }
        }
    }
    CompileTimeContext {
        functions: lowered,
        eligible,
        names,
    }
}

fn compile_time_lowering_span(error: CompileTimeLoweringError, fallback: SourceSpan) -> SourceSpan {
    match error {
        CompileTimeLoweringError::MissingCanonicalType { span }
        | CompileTimeLoweringError::UnsupportedReturnArity { span, .. }
        | CompileTimeLoweringError::UnsupportedConstruct { span, .. }
        | CompileTimeLoweringError::UnknownParameter { span, .. }
        | CompileTimeLoweringError::TypeMismatch { span, .. }
        | CompileTimeLoweringError::InvalidOperatorTypes { span } => span,
        CompileTimeLoweringError::BodyDoesNotProduceSingleResult { span } => {
            span.unwrap_or(fallback)
        }
        CompileTimeLoweringError::UnsupportedResultArity { .. } => fallback,
    }
}

fn direct_calls(expression: &CompileTimeExpression) -> Vec<(FunctionId, SourceSpan)> {
    let mut calls = Vec::new();
    collect_direct_calls(expression, &mut calls);
    calls
}

fn collect_direct_calls(
    expression: &CompileTimeExpression,
    calls: &mut Vec<(FunctionId, SourceSpan)>,
) {
    match expression.kind() {
        CompileTimeExpressionKind::Constant(_)
        | CompileTimeExpressionKind::Parameter(_)
        | CompileTimeExpressionKind::Local(_) => {}
        CompileTimeExpressionKind::Let {
            initializer, body, ..
        } => {
            collect_direct_calls(initializer, calls);
            collect_direct_calls(body, calls);
        }
        CompileTimeExpressionKind::Unary { operand, .. } => collect_direct_calls(operand, calls),
        CompileTimeExpressionKind::Binary { left, right, .. } => {
            collect_direct_calls(left, calls);
            collect_direct_calls(right, calls);
        }
        CompileTimeExpressionKind::Conditional {
            condition,
            when_true,
            when_false,
        } => {
            collect_direct_calls(condition, calls);
            collect_direct_calls(when_true, calls);
            collect_direct_calls(when_false, calls);
        }
        CompileTimeExpressionKind::Tuple(elements) => {
            for element in elements {
                collect_direct_calls(element, calls);
            }
        }
        CompileTimeExpressionKind::Call {
            function,
            arguments,
        } => {
            for argument in arguments {
                collect_direct_calls(argument, calls);
            }
            calls.push((*function, expression.span()));
        }
    }
}

fn evaluate_declaration_defaults(
    declarations: &[HirDeclaration],
    signatures: &BTreeMap<SymbolId, ResolvedFunctionSignature>,
    compile_time: &CompileTimeContext,
    resolver: &mut SignatureResolver<'_>,
    compile_time_evaluations: &mut Vec<FrontEndCompileTimeEvaluation>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    for declaration in declarations {
        match declaration.kind() {
            HirDeclarationKind::Attribute(_) => evaluate_attribute_defaults(
                declaration,
                signatures,
                compile_time,
                resolver,
                compile_time_evaluations,
                diagnostics,
            ),
            HirDeclarationKind::Record(_) => evaluate_record_defaults(
                declaration,
                signatures,
                compile_time,
                resolver,
                compile_time_evaluations,
                diagnostics,
            ),
            HirDeclarationKind::Class(_) => evaluate_class_defaults(
                declaration,
                signatures,
                compile_time,
                resolver,
                compile_time_evaluations,
                diagnostics,
            ),
            HirDeclarationKind::Union(_) => {}
        }
    }
}

fn evaluate_attribute_defaults(
    declaration: &HirDeclaration,
    signatures: &BTreeMap<SymbolId, ResolvedFunctionSignature>,
    compile_time: &CompileTimeContext,
    resolver: &mut SignatureResolver<'_>,
    compile_time_evaluations: &mut Vec<FrontEndCompileTimeEvaluation>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let pending: Vec<_> = resolver
        .attribute_definition(declaration.symbol())
        .into_iter()
        .flat_map(pop_types::AttributeDefinition::parameters)
        .filter_map(|parameter| {
            parameter.pending_default().map(|expression| {
                (
                    parameter.parameter(),
                    expression.clone(),
                    parameter.parameter_type(),
                )
            })
        })
        .collect();
    for (parameter, expression, expected) in pending {
        let evaluated = evaluate_required_expression(
            declaration.module(),
            expression.expression(),
            expected,
            signatures,
            compile_time,
            resolver,
            compile_time_evaluations,
        )
        .and_then(|value| {
            compile_time_attribute_constant(value).ok_or_else(|| {
                vec![compile_time_diagnostics::ineligible_constant_expression(
                    expression.expression().span(),
                    "attribute default",
                )]
            })
        });
        match evaluated {
            Ok(value) => {
                if resolver
                    .install_attribute_parameter_default(declaration.symbol(), parameter, value)
                    .is_err()
                {
                    diagnostics.push(compile_time_diagnostics::ineligible_constant_expression(
                        expression.expression().span(),
                        "attribute default",
                    ));
                }
            }
            Err(errors) => diagnostics.extend(errors),
        }
    }
}

fn evaluate_record_defaults(
    declaration: &HirDeclaration,
    signatures: &BTreeMap<SymbolId, ResolvedFunctionSignature>,
    compile_time: &CompileTimeContext,
    resolver: &mut SignatureResolver<'_>,
    compile_time_evaluations: &mut Vec<FrontEndCompileTimeEvaluation>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let pending: Vec<_> = resolver
        .record_definition(declaration.symbol())
        .into_iter()
        .flat_map(pop_types::RecordDefinition::fields)
        .filter_map(|field| {
            field
                .pending_default()
                .map(|expression| (field.field(), expression.clone(), field.field_type()))
        })
        .collect();
    for (field, expression, expected) in pending {
        install_field_default(
            declaration,
            field,
            &expression,
            expected,
            false,
            signatures,
            compile_time,
            resolver,
            compile_time_evaluations,
            diagnostics,
        );
    }
}

fn evaluate_class_defaults(
    declaration: &HirDeclaration,
    signatures: &BTreeMap<SymbolId, ResolvedFunctionSignature>,
    compile_time: &CompileTimeContext,
    resolver: &mut SignatureResolver<'_>,
    compile_time_evaluations: &mut Vec<FrontEndCompileTimeEvaluation>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let pending: Vec<_> = resolver
        .class_definition(declaration.symbol())
        .into_iter()
        .flat_map(pop_types::ClassDefinition::fields)
        .filter_map(|field| {
            field
                .pending_default()
                .map(|expression| (field.field(), expression.clone(), field.field_type()))
        })
        .collect();
    for (field, expression, expected) in pending {
        install_field_default(
            declaration,
            field,
            &expression,
            expected,
            true,
            signatures,
            compile_time,
            resolver,
            compile_time_evaluations,
            diagnostics,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn install_field_default(
    declaration: &HirDeclaration,
    field: pop_foundation::FieldId,
    pending: &PendingConstantExpression,
    expected: TypeId,
    class_field: bool,
    signatures: &BTreeMap<SymbolId, ResolvedFunctionSignature>,
    compile_time: &CompileTimeContext,
    resolver: &mut SignatureResolver<'_>,
    compile_time_evaluations: &mut Vec<FrontEndCompileTimeEvaluation>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let evaluated = evaluate_required_expression(
        declaration.module(),
        pending.expression(),
        expected,
        signatures,
        compile_time,
        resolver,
        compile_time_evaluations,
    )
    .and_then(|value| {
        compile_time_field_default(value).ok_or_else(|| {
            vec![compile_time_diagnostics::ineligible_constant_expression(
                pending.expression().span(),
                "field default",
            )]
        })
    });
    match evaluated {
        Ok(value) => {
            let installed = if class_field {
                resolver.install_class_field_default(declaration.symbol(), field, value)
            } else {
                resolver.install_record_field_default(declaration.symbol(), field, value)
            };
            if installed.is_err() {
                diagnostics.push(compile_time_diagnostics::ineligible_constant_expression(
                    pending.expression().span(),
                    "field default",
                ));
            }
        }
        Err(errors) => diagnostics.extend(errors),
    }
}

fn evaluate_required_expression(
    module: ModuleId,
    expression: &ExpressionSyntax,
    expected: TypeId,
    signatures: &BTreeMap<SymbolId, ResolvedFunctionSignature>,
    compile_time: &CompileTimeContext,
    resolver: &mut SignatureResolver<'_>,
    compile_time_evaluations: &mut Vec<FrontEndCompileTimeEvaluation>,
) -> Result<CompileTimeValue, Vec<Diagnostic>> {
    let typed = BodyChecker::new(module, resolver, signatures)
        .check_required_expression(expression, expected);
    if !typed.diagnostics().is_empty() {
        return Err(typed.diagnostics().to_vec());
    }
    let Some(typed) = typed.expression() else {
        return Err(vec![
            compile_time_diagnostics::ineligible_constant_expression(
                expression.span(),
                "required constant",
            ),
        ]);
    };
    let lowered = lower_compile_time_expression(typed, resolver.arena()).map_err(|error| {
        vec![compile_time_diagnostics::ineligible_constant_expression(
            compile_time_lowering_span(error, expression.span()),
            "required constant",
        )]
    })?;
    evaluate_compile_time_expression(
        lowered,
        compile_time,
        resolver.arena(),
        compile_time_evaluations,
    )
}

fn evaluate_compile_time_expression(
    expression: CompileTimeExpression,
    context: &CompileTimeContext,
    types: &TypeArena,
    compile_time_evaluations: &mut Vec<FrontEndCompileTimeEvaluation>,
) -> Result<CompileTimeValue, Vec<Diagnostic>> {
    let mut selected = BTreeMap::new();
    collect_reachable_compile_time_functions(&expression, context, &mut selected)?;
    let wrapper = unused_function_id(context);
    let result_type = expression.type_id();
    let span = expression.span();
    let wrapper_function = CompileTimeFunction::new(wrapper, Vec::new(), result_type, expression);
    let mut definitions: Vec<_> = selected.into_values().collect();
    definitions.push(wrapper_function);
    let program = CompileTimeProgram::new(definitions, types)
        .map_err(|error| vec![program_diagnostic(error, span, context)])?;
    let mut eligible = context.eligible.clone();
    eligible.insert(wrapper);
    match CompileTimeInterpreter::new(&program, &eligible, default_compile_time_budget())
        .evaluate_detailed_from(wrapper, &[], span)
    {
        Ok(result) => {
            let value = result.value().clone();
            compile_time_evaluations.push(FrontEndCompileTimeEvaluation::Result(result));
            Ok(value)
        }
        Err(failure) => {
            let diagnostic = evaluation_diagnostic(&failure, context);
            compile_time_evaluations.push(FrontEndCompileTimeEvaluation::Failure(failure));
            Err(vec![diagnostic])
        }
    }
}

fn collect_reachable_compile_time_functions(
    expression: &CompileTimeExpression,
    context: &CompileTimeContext,
    selected: &mut BTreeMap<FunctionId, CompileTimeFunction>,
) -> Result<(), Vec<Diagnostic>> {
    for (function, span) in direct_calls(expression) {
        if !context.eligible.contains(&function) {
            return Err(vec![compile_time_diagnostics::function_not_eligible(
                span,
                compile_time_function_name(&context.names, function),
                [],
            )]);
        }
        if selected.contains_key(&function) {
            continue;
        }
        let Some(definition) = context.functions.get(&function).cloned() else {
            return Err(vec![compile_time_diagnostics::function_not_eligible(
                span,
                compile_time_function_name(&context.names, function),
                [],
            )]);
        };
        selected.insert(function, definition.clone());
        collect_reachable_compile_time_functions(definition.body(), context, selected)?;
    }
    Ok(())
}

fn unused_function_id(context: &CompileTimeContext) -> FunctionId {
    let mut raw = context
        .functions
        .keys()
        .map(|function| function.raw())
        .max()
        .unwrap_or(0)
        .saturating_add(1);
    while context.functions.contains_key(&FunctionId::from_raw(raw)) {
        raw = raw.saturating_add(1);
    }
    FunctionId::from_raw(raw)
}

fn program_diagnostic(
    error: ProgramError,
    span: SourceSpan,
    context: &CompileTimeContext,
) -> Diagnostic {
    match error {
        ProgramError::UnknownFunction(function) => compile_time_diagnostics::function_not_eligible(
            span,
            compile_time_function_name(&context.names, function),
            [],
        ),
        _ => compile_time_diagnostics::ineligible_constant_expression(
            span,
            format!("invalid compile-time program: {error:?}"),
        ),
    }
}

fn evaluation_diagnostic(failure: &EvaluationFailure, context: &CompileTimeContext) -> Diagnostic {
    let origins: Vec<_> = failure
        .call_chain()
        .iter()
        .map(|frame| DiagnosticOrigin::new(frame.call_site(), DiagnosticOriginKind::CompileTime))
        .chain(std::iter::once(DiagnosticOrigin::new(
            failure.origin(),
            DiagnosticOriginKind::CompileTime,
        )))
        .collect();
    let span = failure.location();
    let EvaluationFailureKind::Error(error) = failure.kind() else {
        let cycle = failure
            .call_chain()
            .iter()
            .map(|frame| compile_time_function_name(&context.names, frame.function()))
            .collect::<Vec<_>>()
            .join(" -> ");
        return compile_time_diagnostics::cycle(span, cycle, origins);
    };
    let diagnostic = match error {
        EvaluationError::UnknownFunction(function)
        | EvaluationError::IneligibleFunction(function) => {
            compile_time_diagnostics::function_not_eligible(
                span,
                compile_time_function_name(&context.names, function),
                [],
            )
        }
        EvaluationError::IntegerOverflow => {
            compile_time_diagnostics::constant_integer_overflow(span, "required constant")
        }
        EvaluationError::DivisionByZero => {
            compile_time_diagnostics::constant_division_by_zero(span, "required constant")
        }
        EvaluationError::Budget(resource) => compile_time_diagnostics::resource_limit(
            span,
            budget_name(resource),
            budget_limit(resource, *failure.budget()),
            [],
        ),
        EvaluationError::WrongArity { .. } | EvaluationError::TypeMismatch => {
            compile_time_diagnostics::ineligible_constant_expression(span, "required constant")
        }
    };
    origins
        .into_iter()
        .fold(diagnostic, Diagnostic::with_origin)
}

const fn default_compile_time_budget() -> CompileTimeBudget {
    CompileTimeBudget::new(
        QueryBudget::new(100_000, 1_048_576, 128),
        65_536,
        1_048_576,
        128,
    )
}

const fn budget_name(error: BudgetError) -> &'static str {
    match error {
        BudgetError::InstructionLimit => "instructions",
        BudgetError::AllocationLimit => "allocation bytes",
        BudgetError::CallDepthLimit => "call depth",
        BudgetError::LiveValueLimit => "live values",
        BudgetError::OutputSizeLimit => "output bytes",
        BudgetError::DiagnosticLimit => "diagnostics",
        BudgetError::UnbalancedCallExit => "call stack",
    }
}

const fn budget_limit(error: BudgetError, budget: CompileTimeBudget) -> u64 {
    match error {
        BudgetError::InstructionLimit => budget.query().instruction_fuel(),
        BudgetError::AllocationLimit => budget.query().allocation_bytes(),
        BudgetError::CallDepthLimit => budget.query().maximum_call_depth() as u64,
        BudgetError::LiveValueLimit => budget.maximum_live_values(),
        BudgetError::OutputSizeLimit => budget.maximum_output_bytes(),
        BudgetError::DiagnosticLimit => budget.maximum_diagnostics(),
        BudgetError::UnbalancedCallExit => 0,
    }
}

fn compile_time_function_name(
    names: &BTreeMap<FunctionId, String>,
    function: FunctionId,
) -> String {
    names
        .get(&function)
        .cloned()
        .unwrap_or_else(|| format!("function#{}", function.raw()))
}

fn compile_time_attribute_constant(value: CompileTimeValue) -> Option<AttributeConstant> {
    match value {
        CompileTimeValue::Nil => Some(AttributeConstant::Nil),
        CompileTimeValue::Boolean(value) => Some(AttributeConstant::Boolean(value)),
        CompileTimeValue::Integer(value) => Some(AttributeConstant::Integer(value)),
        CompileTimeValue::Float(value) => Some(AttributeConstant::Float(value)),
        CompileTimeValue::String(value) => Some(AttributeConstant::String(value)),
        CompileTimeValue::Tuple(values) => values
            .into_iter()
            .map(compile_time_attribute_constant)
            .collect::<Option<Vec<_>>>()
            .map(AttributeConstant::Tuple),
        CompileTimeValue::Array(_)
        | CompileTimeValue::Record(_)
        | CompileTimeValue::Union { .. }
        | CompileTimeValue::TypeReference(_)
        | CompileTimeValue::SymbolReference(_) => None,
    }
}

fn compile_time_field_default(value: CompileTimeValue) -> Option<FieldDefault> {
    match value {
        CompileTimeValue::Nil => Some(FieldDefault::Nil),
        CompileTimeValue::Boolean(value) => Some(FieldDefault::Boolean(value)),
        CompileTimeValue::Integer(value) => Some(FieldDefault::Integer(value)),
        CompileTimeValue::Float(value) => Some(FieldDefault::Float(value)),
        CompileTimeValue::String(value) => Some(FieldDefault::String(value)),
        CompileTimeValue::Tuple(_)
        | CompileTimeValue::Array(_)
        | CompileTimeValue::Record(_)
        | CompileTimeValue::Union { .. }
        | CompileTimeValue::TypeReference(_)
        | CompileTimeValue::SymbolReference(_) => None,
    }
}

fn refresh_declarations(
    declarations: Vec<HirDeclaration>,
    resolver: &SignatureResolver<'_>,
) -> Vec<HirDeclaration> {
    declarations
        .into_iter()
        .map(|declaration| {
            if matches!(declaration.kind(), HirDeclarationKind::Attribute(_)) {
                if let Some(definition) = resolver.attribute_definition(declaration.symbol()) {
                    return HirDeclaration::attribute(
                        declaration.module(),
                        declaration.bubble(),
                        declaration.visibility(),
                        declaration.name(),
                        definition,
                    );
                }
            }
            if matches!(declaration.kind(), HirDeclarationKind::Record(_)) {
                if let Some(definition) = resolver.record_definition(declaration.symbol()) {
                    return HirDeclaration::record(
                        declaration.module(),
                        declaration.bubble(),
                        declaration.visibility(),
                        declaration.name(),
                        definition,
                    );
                }
            }
            if matches!(declaration.kind(), HirDeclarationKind::Class(_)) {
                if let Some(definition) = resolver.class_definition(declaration.symbol()) {
                    return HirDeclaration::class(
                        declaration.module(),
                        declaration.bubble(),
                        declaration.visibility(),
                        declaration.name(),
                        definition,
                    );
                }
            }
            declaration
        })
        .collect()
}

fn resolve_symbol(
    database: &ResolutionDatabase,
    module: ModuleId,
    name: &str,
    space: SymbolSpace,
    span: SourceSpan,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<SymbolId> {
    let result = database.resolve(module, name, space, span);
    diagnostics.extend(result.diagnostics().iter().cloned());
    result.symbol()
}

fn syntax_error(span: SourceSpan, expectation: &'static str) -> Diagnostic {
    syntax_diagnostics::unexpected_token(span, expectation, "malformed declaration")
}

fn sort_diagnostics(diagnostics: &mut Vec<Diagnostic>) {
    diagnostics.sort_by_key(|diagnostic| {
        let span = diagnostic.primary_span();
        (
            span.file(),
            span.range().start(),
            diagnostic.code().as_str(),
        )
    });
    diagnostics.dedup();
}

fn diagnostic_snapshot(diagnostics: &[Diagnostic]) -> String {
    let mut snapshot = String::new();
    for diagnostic in diagnostics {
        let range = diagnostic.primary_span().range();
        snapshot.push_str(diagnostic.code().as_str());
        snapshot.push('@');
        snapshot.push_str(&range.start().to_u32().to_string());
        snapshot.push_str("..");
        snapshot.push_str(&range.end().to_u32().to_string());
        snapshot.push('\n');
    }
    snapshot
}
