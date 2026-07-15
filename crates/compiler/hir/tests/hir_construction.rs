#![allow(clippy::too_many_lines)]

use std::collections::{BTreeMap, BTreeSet};

use pop_foundation::{BubbleId, FileId, ModuleId, NamespaceId, SymbolId};
use pop_hir::{
    HirBubble, HirCallDispatch, HirCaptureMode, HirCaptureSource, HirDeclaration,
    HirExpressionKind, HirFunctionContext, HirKnownCallables, HirStatementKind,
    HirVerificationError, build_hir_function, build_hir_function_with_attributes,
    build_hir_function_with_known_callables_and_attributes, build_hir_method, verify_hir_function,
};
use pop_resolve::{
    ModuleInput, ResolutionDatabase, SymbolSpace, Visibility, build_declaration_index,
};
use pop_source::SourceFile;
use pop_syntax::{
    NodeKind, parse_attribute_declaration, parse_attribute_use, parse_class_declaration,
    parse_class_method_body, parse_file, parse_function_body, parse_function_signature,
    parse_interface_declaration, parse_record_declaration, parse_union_declaration,
};
use pop_types::{
    BodyChecker, ResolvedFunctionSignature, SignatureResolver, TypeArena, TypedBody,
    embedded_bootstrap_schema,
};

struct TypedFixture {
    arena: TypeArena,
    functions: Vec<(ResolvedFunctionSignature, TypedBody, Visibility)>,
}

fn node(syntax: &pop_syntax::SyntaxTree, kind: NodeKind) -> &pop_syntax::SyntaxNode {
    syntax
        .root()
        .children()
        .iter()
        .find(|node| node.kind() == kind)
        .expect("declaration node")
}

fn resolve_test_attribute(
    resolver: &mut SignatureResolver<'_>,
    module: ModuleId,
    symbol: SymbolId,
    source: &SourceFile,
    syntax: &pop_syntax::SyntaxTree,
) -> pop_types::ResolvedAttribute {
    let declaration =
        parse_attribute_declaration(source, syntax, node(syntax, NodeKind::AttributeDeclaration))
            .expect("attribute declaration");
    let attribute_use = parse_attribute_use(source, syntax, node(syntax, NodeKind::AttributeUse))
        .expect("attribute use");
    assert!(
        resolver
            .define_attribute(module, symbol, &declaration)
            .diagnostics()
            .is_empty()
    );
    resolver
        .resolve_attribute_use(module, &attribute_use)
        .attribute()
        .expect("resolved attribute")
        .clone()
}

fn typed_fixture() -> TypedFixture {
    let module = ModuleId::from_raw(0);
    let bubble = BubbleId::from_raw(4);
    let source = SourceFile::new(
        FileId::from_raw(0),
        "src/calculation.pop",
        "namespace Example\n\
         private function combine(left: Int, right: Int): Int\n\
             return left + right\n\
         end\n\
         public function calculate(left: Int, right: Int): Int\n\
             local sum = combine(left, right)\n\
             if left < right then\n\
                 return sum\n\
             else\n\
                 return right\n\
             end\n\
         end\n\
         private function optional(value: String?, fallback: String): String?\n\
             local selected = value ?? fallback\n\
             local propagated = value?\n\
             if local bound = value then\n\
                 local copy = bound\n\
             end\n\
             while local bound = value do\n\
                 local copy = bound\n\
                 break\n\
             end\n\
             if value ~= nil then\n\
                 local narrowed = value\n\
             end\n\
             return value\n\
         end\n\
         private function result(input: Result<Int, String>): Result<String, String>\n\
             defer\n\
                 print(\"cleanup\")\n\
             end\n\
             local value = try input\n\
             return Result.Ok(String(value))\n\
         end\n\
         private async function load(value: Int): Int\n\
             return value\n\
         end\n\
         private async function consume(): Int\n\
             return await load(42)\n\
         end\n",
    )
    .expect("source");
    let syntax = parse_file(&source);
    let parsed: Vec<_> = syntax
        .root()
        .children()
        .iter()
        .filter(|node| node.kind() == NodeKind::FunctionDeclaration)
        .map(|node| {
            let signature = parse_function_signature(&source, &syntax, node).expect("signature");
            let body = parse_function_body(&source, &syntax, node, &signature).expect("body");
            (signature, body)
        })
        .collect();
    let indexed = build_declaration_index(&[ModuleInput::new(module, bubble, &source, &syntax)]);
    let mut symbols = BTreeMap::new();
    let mut visibilities = BTreeMap::new();
    for (signature, _) in &parsed {
        let qualified = format!("Example.{}", signature.name());
        let declaration = indexed
            .index()
            .declaration_by_qualified_name(&qualified, SymbolSpace::Value)[0];
        symbols.insert(signature.name().to_owned(), declaration.symbol());
        visibilities.insert(declaration.symbol(), declaration.visibility());
    }
    let database = ResolutionDatabase::new(indexed.into_index());
    let mut resolver =
        SignatureResolver::new(&database, embedded_bootstrap_schema().expect("bootstrap"));
    let mut signatures = BTreeMap::new();
    for (syntax_signature, _) in &parsed {
        let symbol = symbols[syntax_signature.name()];
        let result = resolver.resolve(module, symbol, syntax_signature);
        signatures.insert(
            symbol,
            result.signature().expect("resolved signature").clone(),
        );
    }
    let functions = parsed
        .iter()
        .map(|(syntax_signature, body)| {
            let symbol = symbols[syntax_signature.name()];
            let typed = BodyChecker::new(module, &mut resolver, &signatures)
                .check(&signatures[&symbol], body);
            (
                signatures[&symbol].clone(),
                typed.body().expect("typed body").clone(),
                visibilities[&symbol],
            )
        })
        .collect();
    TypedFixture {
        arena: resolver.into_arena(),
        functions,
    }
}

#[test]
fn construction_retains_async_identity_and_typed_await_until_mir() {
    let fixture = typed_fixture();
    let known: BTreeSet<_> = fixture
        .functions
        .iter()
        .map(|(signature, _, _)| signature.symbol())
        .collect();
    let (signature, body, visibility) = fixture
        .functions
        .iter()
        .find(|(signature, _, _)| signature.name() == "consume")
        .expect("consume");
    let hir = build_hir_function(
        ModuleId::from_raw(0),
        BubbleId::from_raw(4),
        *visibility,
        signature,
        body,
        &fixture.arena,
        &known,
    )
    .expect("verified async HIR");

    assert!(hir.is_async());
    let HirStatementKind::Return { values } = hir.body()[0].kind() else {
        panic!("async HIR return");
    };
    assert!(matches!(
        values[0].kind(),
        HirExpressionKind::Await { task }
            if matches!(task.kind(), HirExpressionKind::Call { .. })
    ));
}

#[test]
fn construction_preserves_static_types_owners_and_direct_dispatch() {
    let fixture = typed_fixture();
    let known: BTreeSet<_> = fixture
        .functions
        .iter()
        .map(|(signature, _, _)| signature.symbol())
        .collect();
    let (signature, body, visibility) = fixture
        .functions
        .iter()
        .find(|(signature, _, _)| signature.name() == "calculate")
        .expect("calculate");
    let hir = build_hir_function(
        ModuleId::from_raw(0),
        BubbleId::from_raw(4),
        *visibility,
        signature,
        body,
        &fixture.arena,
        &known,
    )
    .expect("verified HIR");

    assert_eq!(hir.module(), ModuleId::from_raw(0));
    assert_eq!(hir.bubble(), BubbleId::from_raw(4));
    assert_eq!(hir.symbol(), signature.symbol());
    let HirStatementKind::Local { initializer, .. } = hir.body()[0].kind() else {
        panic!("HIR local");
    };
    assert!(matches!(
        initializer.kind(),
        HirExpressionKind::Call {
            dispatch: HirCallDispatch::Direct { function },
            ..
        } if *function == fixture.functions[0].0.symbol()
    ));
    assert!(verify_hir_function(&hir, &fixture.arena, &known).is_ok());
}

#[test]
fn construction_retains_typed_optional_control_until_mir() {
    // ADR 0051: HIR keeps optional control distinct so MIR lowering, rather
    // than a backend, owns presence tests, typed extraction, and lazy edges.
    let fixture = typed_fixture();
    let known: BTreeSet<_> = fixture
        .functions
        .iter()
        .map(|(signature, _, _)| signature.symbol())
        .collect();
    let (signature, body, visibility) = fixture
        .functions
        .iter()
        .find(|(signature, _, _)| signature.name() == "optional")
        .expect("optional");
    let hir = build_hir_function(
        ModuleId::from_raw(0),
        BubbleId::from_raw(4),
        *visibility,
        signature,
        body,
        &fixture.arena,
        &known,
    )
    .expect("verified optional HIR");

    assert!(matches!(
        hir.body()[0].kind(),
        HirStatementKind::Local { initializer, .. }
            if matches!(initializer.kind(), HirExpressionKind::OptionalDefault { .. })
    ));
    assert!(matches!(
        hir.body()[1].kind(),
        HirStatementKind::Local { initializer, .. }
            if matches!(initializer.kind(), HirExpressionKind::OptionalPropagate { .. })
    ));
    assert!(matches!(
        hir.body()[2].kind(),
        HirStatementKind::OptionalIf { then_body, .. } if then_body.len() == 1
    ));
    assert!(matches!(
        hir.body()[3].kind(),
        HirStatementKind::OptionalWhile { body, .. } if body.len() == 2
    ));
    assert!(verify_hir_function(&hir, &fixture.arena, &known).is_ok());
}

#[test]
fn construction_retains_result_failure_and_cleanup_scopes_until_mir() {
    let fixture = typed_fixture();
    let known: BTreeSet<_> = fixture
        .functions
        .iter()
        .map(|(signature, _, _)| signature.symbol())
        .collect();
    let (signature, body, visibility) = fixture
        .functions
        .iter()
        .find(|(signature, _, _)| signature.name() == "result")
        .expect("result");
    let hir = build_hir_function(
        ModuleId::from_raw(0),
        BubbleId::from_raw(4),
        *visibility,
        signature,
        body,
        &fixture.arena,
        &known,
    )
    .expect("verified result HIR");

    assert!(matches!(
        hir.body()[0].kind(),
        HirStatementKind::Defer { .. }
    ));
    let HirStatementKind::Local { initializer, .. } = hir.body()[1].kind() else {
        panic!("result propagation local");
    };
    assert!(matches!(
        initializer.kind(),
        HirExpressionKind::ResultPropagate { .. }
    ));
    let HirStatementKind::Return { values } = hir.body()[2].kind() else {
        panic!("result return");
    };
    assert!(matches!(
        values[0].kind(),
        HirExpressionKind::ResultCase { .. }
    ));
}

#[test]
fn bubble_derives_sorted_public_symbols_and_has_a_deterministic_dump() {
    let fixture = typed_fixture();
    let known: BTreeSet<SymbolId> = fixture
        .functions
        .iter()
        .map(|(signature, _, _)| signature.symbol())
        .collect();
    let functions: Vec<_> = fixture
        .functions
        .iter()
        .rev()
        .map(|(signature, body, visibility)| {
            build_hir_function(
                ModuleId::from_raw(0),
                BubbleId::from_raw(4),
                *visibility,
                signature,
                body,
                &fixture.arena,
                &known,
            )
            .expect("HIR")
        })
        .collect();
    let bubble = HirBubble::new(
        BubbleId::from_raw(4),
        NamespaceId::from_raw(2),
        vec![
            BubbleId::from_raw(9),
            BubbleId::from_raw(1),
            BubbleId::from_raw(9),
        ],
        functions,
    )
    .expect("consistent owners");

    assert_eq!(
        bubble.dependencies(),
        &[BubbleId::from_raw(1), BubbleId::from_raw(9)]
    );
    assert_eq!(bubble.public_symbols().len(), 1);
    assert_eq!(bubble.functions()[0].symbol().raw(), 0);
    let dump = bubble.dump(&fixture.arena);
    assert_eq!(dump, bubble.dump(&fixture.arena));
    assert!(dump.contains("hir bubble b4 namespace n2"));
    assert!(dump.contains("call.direct s0"));
    assert!(!dump.to_ascii_lowercase().contains("dynamic"));
    assert!(!dump.to_ascii_lowercase().contains("llvm"));
    assert!(bubble.verify(&fixture.arena).is_ok());
}

#[test]
fn record_construction_selection_and_updates_survive_hir_lowering() {
    let module = ModuleId::from_raw(0);
    let bubble = BubbleId::from_raw(0);
    let source = SourceFile::new(
        FileId::from_raw(0),
        "src/player.pop",
        "namespace Example\n\
         public attribute Serializable(version: UInt32 = 1)\n\
         @Serializable(version = 2)\n\
         public record Player\n\
             name: String\n\
             score: Int\n\
         end\n\
         public function update(player: Player): Player\n\
             local value: Player = { name = \"Ana\", score = player.score, }\n\
             return value with { score = 2, }\n\
         end\n",
    )
    .expect("source");
    let syntax = parse_file(&source);
    let record_node = node(&syntax, NodeKind::RecordDeclaration);
    let function_node = node(&syntax, NodeKind::FunctionDeclaration);
    let record_syntax = parse_record_declaration(&source, &syntax, record_node).expect("record");
    let function_syntax =
        parse_function_signature(&source, &syntax, function_node).expect("signature");
    let body_syntax =
        parse_function_body(&source, &syntax, function_node, &function_syntax).expect("body");
    let indexed = build_declaration_index(&[ModuleInput::new(module, bubble, &source, &syntax)]);
    let record_symbol = indexed
        .index()
        .declaration_by_qualified_name("Example.Player", SymbolSpace::Type)[0]
        .symbol();
    let attribute_symbol = indexed
        .index()
        .declaration_by_qualified_name("Example.Serializable", SymbolSpace::Type)[0]
        .symbol();
    let function_symbol = indexed
        .index()
        .declaration_by_qualified_name("Example.update", SymbolSpace::Value)[0]
        .symbol();
    let database = ResolutionDatabase::new(indexed.into_index());
    let mut resolver =
        SignatureResolver::new(&database, embedded_bootstrap_schema().expect("bootstrap"));
    assert!(
        resolver
            .define_record(module, record_symbol, &record_syntax)
            .diagnostics()
            .is_empty()
    );
    let resolved_attribute =
        resolve_test_attribute(&mut resolver, module, attribute_symbol, &source, &syntax);
    let signature = resolver
        .resolve(module, function_symbol, &function_syntax)
        .signature()
        .expect("signature")
        .clone();
    let signatures = BTreeMap::from([(function_symbol, signature.clone())]);
    let typed =
        BodyChecker::new(module, &mut resolver, &signatures).check(&signature, &body_syntax);
    let known = BTreeSet::from([function_symbol]);
    let hir = build_hir_function_with_attributes(
        HirFunctionContext::new(module, bubble, Visibility::Public),
        &signature,
        typed.body().expect("typed body"),
        resolver.arena(),
        &known,
        std::slice::from_ref(&resolved_attribute),
    )
    .expect("HIR");

    let HirStatementKind::Local { initializer, .. } = hir.body()[0].kind() else {
        panic!("local");
    };
    assert!(matches!(
        initializer.kind(),
        HirExpressionKind::Record { .. }
    ));
    assert_eq!(hir.attributes().len(), 1);
    assert_eq!(
        hir.attributes()[0].attribute(),
        resolved_attribute.attribute()
    );
    assert_eq!(hir.attributes()[0].span(), resolved_attribute.span());
    let HirStatementKind::Return { values } = hir.body()[1].kind() else {
        panic!("return");
    };
    assert!(matches!(
        values[0].kind(),
        HirExpressionKind::RecordUpdate { .. }
    ));
    assert!(verify_hir_function(&hir, resolver.arena(), &known).is_ok());
}

#[test]
fn tagged_union_cases_survive_hir_with_stable_case_ids() {
    let module = ModuleId::from_raw(0);
    let bubble = BubbleId::from_raw(0);
    let source = SourceFile::new(
        FileId::from_raw(0),
        "src/state.pop",
        "namespace Example\n\
         public union LoadState\n\
             Idle\n\
             Loading(progress: Int)\n\
         end\n\
         public function loading(progress: Int): LoadState\n\
             return LoadState.Loading(progress)\n\
         end\n",
    )
    .expect("source");
    let syntax = parse_file(&source);
    let union_node = syntax
        .root()
        .children()
        .iter()
        .find(|node| node.kind() == NodeKind::UnionDeclaration)
        .expect("union");
    let function_node = syntax
        .root()
        .children()
        .iter()
        .find(|node| node.kind() == NodeKind::FunctionDeclaration)
        .expect("function");
    let union_syntax = parse_union_declaration(&source, &syntax, union_node).expect("union");
    let function_syntax =
        parse_function_signature(&source, &syntax, function_node).expect("signature");
    let body_syntax =
        parse_function_body(&source, &syntax, function_node, &function_syntax).expect("body");
    let indexed = build_declaration_index(&[ModuleInput::new(module, bubble, &source, &syntax)]);
    let union_symbol = indexed
        .index()
        .declaration_by_qualified_name("Example.LoadState", SymbolSpace::Type)[0]
        .symbol();
    let function_symbol = indexed
        .index()
        .declaration_by_qualified_name("Example.loading", SymbolSpace::Value)[0]
        .symbol();
    let database = ResolutionDatabase::new(indexed.into_index());
    let mut resolver =
        SignatureResolver::new(&database, embedded_bootstrap_schema().expect("bootstrap"));
    let definition = resolver.define_union(module, union_symbol, &union_syntax);
    let loading_case = definition.definition().expect("definition").cases()[1].case();
    let signature = resolver
        .resolve(module, function_symbol, &function_syntax)
        .signature()
        .expect("signature")
        .clone();
    let signatures = BTreeMap::from([(function_symbol, signature.clone())]);
    let typed =
        BodyChecker::new(module, &mut resolver, &signatures).check(&signature, &body_syntax);
    let known = BTreeSet::from([function_symbol]);
    let hir = build_hir_function(
        module,
        bubble,
        Visibility::Public,
        &signature,
        typed.body().expect("typed body"),
        resolver.arena(),
        &known,
    )
    .expect("HIR");
    let HirStatementKind::Return { values } = hir.body()[0].kind() else {
        panic!("return");
    };
    assert!(matches!(
        values[0].kind(),
        HirExpressionKind::UnionCase { case, .. } if *case == loading_case
    ));
    assert!(verify_hir_function(&hir, resolver.arena(), &known).is_ok());
}

#[test]
fn typed_collections_survive_hir_in_source_evaluation_order() {
    let module = ModuleId::from_raw(0);
    let bubble = BubbleId::from_raw(0);
    let source = SourceFile::new(
        FileId::from_raw(0),
        "src/collections.pop",
        "namespace Example\n\
         public function collections(): ({String}, {[String]: Int})\n\
             local names: {String} = { \"first\", \"second\" }\n\
             local scores: {[String]: Int} = { first = 1, second = 2 }\n\
             names[2] = \"updated\"\n\
             local firstName: String? = names[1]\n\
             local numbers = Array.create<<Int>>(4, 0)\n\
             Array.fill(numbers, 7)\n\
             local count = Array.length(numbers)\n\
             local first = Array.get(numbers, 1)\n\
             local values = List.withCapacity<<Int>>(4)\n\
             List.add(values, first)\n\
             local maybeValue = values[1]\n\
             local value = List.get(values, 1)\n\
             values[1] = value\n\
             local listCount = List.length(values)\n\
             return (names, scores)\n\
         end\n",
    )
    .expect("source");
    let syntax = parse_file(&source);
    let function_node = node(&syntax, NodeKind::FunctionDeclaration);
    let function_syntax =
        parse_function_signature(&source, &syntax, function_node).expect("signature");
    let body_syntax =
        parse_function_body(&source, &syntax, function_node, &function_syntax).expect("body");
    let indexed = build_declaration_index(&[ModuleInput::new(module, bubble, &source, &syntax)]);
    let function_symbol = indexed
        .index()
        .declaration_by_qualified_name("Example.collections", SymbolSpace::Value)[0]
        .symbol();
    let database = ResolutionDatabase::new(indexed.into_index());
    let mut resolver =
        SignatureResolver::new(&database, embedded_bootstrap_schema().expect("bootstrap"));
    let signature = resolver
        .resolve(module, function_symbol, &function_syntax)
        .signature()
        .expect("signature")
        .clone();
    let signatures = BTreeMap::from([(function_symbol, signature.clone())]);
    let typed =
        BodyChecker::new(module, &mut resolver, &signatures).check(&signature, &body_syntax);
    let known = BTreeSet::from([function_symbol]);
    let hir = build_hir_function(
        module,
        bubble,
        Visibility::Public,
        &signature,
        typed.body().expect("typed body"),
        resolver.arena(),
        &known,
    )
    .expect("HIR");

    let HirStatementKind::Local { initializer, .. } = hir.body()[0].kind() else {
        panic!("array local");
    };
    assert!(matches!(
        initializer.kind(),
        HirExpressionKind::Array(elements)
            if matches!(elements[0].kind(), HirExpressionKind::String(value) if value == "first")
                && matches!(elements[1].kind(), HirExpressionKind::String(value) if value == "second")
    ));
    let HirStatementKind::Local { initializer, .. } = hir.body()[1].kind() else {
        panic!("table local");
    };
    assert!(matches!(
        initializer.kind(),
        HirExpressionKind::Table(entries)
            if matches!(entries[0].key().kind(), HirExpressionKind::String(value) if value == "first")
                && matches!(entries[1].value().kind(), HirExpressionKind::Integer(value) if value.to_string() == "2")
    ));
    assert!(matches!(
        hir.body()[2].kind(),
        HirStatementKind::ArraySet { array, index, value }
            if matches!(array.kind(), HirExpressionKind::Local(_))
                && matches!(index.kind(), HirExpressionKind::Integer(value) if value.to_string() == "2")
                && matches!(value.kind(), HirExpressionKind::String(value) if value == "updated")
    ));
    let HirStatementKind::Local { initializer, .. } = hir.body()[3].kind() else {
        panic!("indexed local");
    };
    assert!(matches!(
        initializer.kind(),
        HirExpressionKind::ArrayGet { array, index }
            if matches!(array.kind(), HirExpressionKind::Local(_))
                && matches!(index.kind(), HirExpressionKind::Integer(value) if value.to_string() == "1")
    ));
    assert!(verify_hir_function(&hir, resolver.arena(), &known).is_ok());
    assert!(matches!(
        hir.body()[4].kind(),
        HirStatementKind::Local { initializer, .. }
            if matches!(initializer.kind(), HirExpressionKind::ArrayCreate { .. })
    ));
    assert!(matches!(
        hir.body()[5].kind(),
        HirStatementKind::Expression(expression)
            if matches!(expression.kind(), HirExpressionKind::ArrayFill { .. })
    ));
    assert!(matches!(
        hir.body()[6].kind(),
        HirStatementKind::Local { initializer, .. }
            if matches!(initializer.kind(), HirExpressionKind::ArrayLength { .. })
    ));
    assert!(matches!(
        hir.body()[7].kind(),
        HirStatementKind::Local { initializer, .. }
            if matches!(initializer.kind(), HirExpressionKind::ArrayGetChecked { .. })
    ));
    assert!(matches!(
        hir.body()[8].kind(),
        HirStatementKind::Local { initializer, .. }
            if matches!(initializer.kind(), HirExpressionKind::ListCreate { capacity: Some(_) })
    ));
    assert!(matches!(
        hir.body()[9].kind(),
        HirStatementKind::Expression(expression)
            if matches!(expression.kind(), HirExpressionKind::ListAdd { .. })
    ));
    assert!(matches!(
        hir.body()[10].kind(),
        HirStatementKind::Local { initializer, .. }
            if matches!(initializer.kind(), HirExpressionKind::ListGet { .. })
    ));
    assert!(matches!(
        hir.body()[11].kind(),
        HirStatementKind::Local { initializer, .. }
            if matches!(initializer.kind(), HirExpressionKind::ListGetChecked { .. })
    ));
    assert!(matches!(
        hir.body()[12].kind(),
        HirStatementKind::ListSet { .. }
    ));
    assert!(matches!(
        hir.body()[13].kind(),
        HirStatementKind::Local { initializer, .. }
            if matches!(initializer.kind(), HirExpressionKind::ListLength { .. })
    ));
}

#[test]
fn closures_retain_binding_identity_capture_mode_and_nested_typed_body() {
    let module = ModuleId::from_raw(0);
    let bubble = BubbleId::from_raw(0);
    let source = SourceFile::new(
        FileId::from_raw(0),
        "src/closures.pop",
        "namespace Example\n\
         public function make(): function(delta: Int): Int\n\
             local total = 0\n\
             local function add(delta: Int): Int\n\
                 total = total + delta\n\
                 return total\n\
             end\n\
             return add\n\
         end\n",
    )
    .expect("source");
    let syntax = parse_file(&source);
    let function_node = node(&syntax, NodeKind::FunctionDeclaration);
    let function_syntax =
        parse_function_signature(&source, &syntax, function_node).expect("signature");
    let body_syntax =
        parse_function_body(&source, &syntax, function_node, &function_syntax).expect("body");
    let indexed = build_declaration_index(&[ModuleInput::new(module, bubble, &source, &syntax)]);
    let function_symbol = indexed
        .index()
        .declaration_by_qualified_name("Example.make", SymbolSpace::Value)[0]
        .symbol();
    let database = ResolutionDatabase::new(indexed.into_index());
    let mut resolver =
        SignatureResolver::new(&database, embedded_bootstrap_schema().expect("bootstrap"));
    let signature = resolver
        .resolve(module, function_symbol, &function_syntax)
        .signature()
        .expect("signature")
        .clone();
    let signatures = BTreeMap::from([(function_symbol, signature.clone())]);
    let typed =
        BodyChecker::new(module, &mut resolver, &signatures).check(&signature, &body_syntax);
    assert!(
        typed.diagnostics().is_empty(),
        "{}",
        typed.diagnostic_snapshot()
    );
    let known = BTreeSet::from([function_symbol]);
    let hir = build_hir_function(
        module,
        bubble,
        Visibility::Public,
        &signature,
        typed.body().expect("typed body"),
        resolver.arena(),
        &known,
    )
    .expect("closure HIR");

    let HirStatementKind::Local {
        binding: total_binding,
        ..
    } = hir.body()[0].kind()
    else {
        panic!("captured local");
    };
    let HirStatementKind::Local { initializer, .. } = hir.body()[1].kind() else {
        panic!("local closure");
    };
    let HirExpressionKind::Closure(closure) = initializer.kind() else {
        panic!("closure expression");
    };
    assert_eq!(closure.captures().len(), 1);
    let capture = &closure.captures()[0];
    assert_eq!(capture.binding(), *total_binding);
    assert_eq!(capture.mode(), HirCaptureMode::Cell);
    assert!(matches!(capture.source(), HirCaptureSource::Local(_)));
    assert!(matches!(
        closure.body()[0].kind(),
        HirStatementKind::CaptureSet { capture: found, .. } if *found == capture.capture()
    ));

    let bubble_hir =
        HirBubble::new(bubble, NamespaceId::from_raw(0), Vec::new(), vec![hir]).expect("Bubble");
    assert!(bubble_hir.verify(resolver.arena()).is_ok());
    let dump = bubble_hir.dump(resolver.arena());
    assert!(dump.contains("closure nested#0"));
    assert!(dump.contains("capture.cell"));
    assert!(!dump.to_ascii_lowercase().contains("table environment"));
}

#[test]
fn exhaustive_matches_retain_resolved_case_ids_and_typed_payload_bindings() {
    let module = ModuleId::from_raw(0);
    let bubble = BubbleId::from_raw(0);
    let source = SourceFile::new(
        FileId::from_raw(0),
        "src/match.pop",
        "namespace Example\n\
         public union ResultValue\n\
             Ok(value: Int)\n\
             Error(message: String)\n\
         end\n\
         public function consume(result: ResultValue): Int\n\
             match result\n\
             when ResultValue.Ok(value) then\n\
                 return value\n\
             when ResultValue.Error(_) then\n\
                 return 0\n\
             end\n\
         end\n",
    )
    .expect("source");
    let syntax = parse_file(&source);
    let union_node = node(&syntax, NodeKind::UnionDeclaration);
    let function_node = node(&syntax, NodeKind::FunctionDeclaration);
    let union_syntax = parse_union_declaration(&source, &syntax, union_node).expect("union");
    let function_syntax =
        parse_function_signature(&source, &syntax, function_node).expect("signature");
    let body_syntax =
        parse_function_body(&source, &syntax, function_node, &function_syntax).expect("body");
    let indexed = build_declaration_index(&[ModuleInput::new(module, bubble, &source, &syntax)]);
    let union_symbol = indexed
        .index()
        .declaration_by_qualified_name("Example.ResultValue", SymbolSpace::Type)[0]
        .symbol();
    let function_symbol = indexed
        .index()
        .declaration_by_qualified_name("Example.consume", SymbolSpace::Value)[0]
        .symbol();
    let database = ResolutionDatabase::new(indexed.into_index());
    let mut resolver =
        SignatureResolver::new(&database, embedded_bootstrap_schema().expect("bootstrap"));
    let union = resolver
        .define_union(module, union_symbol, &union_syntax)
        .definition()
        .expect("union definition")
        .clone();
    let signature = resolver
        .resolve(module, function_symbol, &function_syntax)
        .signature()
        .expect("signature")
        .clone();
    let signatures = BTreeMap::from([(function_symbol, signature.clone())]);
    let typed =
        BodyChecker::new(module, &mut resolver, &signatures).check(&signature, &body_syntax);
    assert!(
        typed.diagnostics().is_empty(),
        "{}",
        typed.diagnostic_snapshot()
    );
    let known = BTreeSet::from([function_symbol]);
    let hir = build_hir_function(
        module,
        bubble,
        Visibility::Public,
        &signature,
        typed.body().expect("typed body"),
        resolver.arena(),
        &known,
    )
    .expect("match HIR");

    let HirStatementKind::Match {
        union: found_union,
        arms,
        ..
    } = hir.body()[0].kind()
    else {
        panic!("HIR match");
    };
    assert_eq!(*found_union, union_symbol);
    assert_eq!(arms.len(), union.cases().len());
    assert_eq!(arms[0].case(), union.cases()[0].case());
    assert_eq!(arms[1].case(), union.cases()[1].case());
    assert!(arms[0].bindings()[0].local().is_some());
    assert!(arms[1].bindings()[0].is_ignored());

    let declaration =
        HirDeclaration::tagged_union(module, bubble, Visibility::Public, "ResultValue", &union);
    let bubble_hir = HirBubble::new_with_declarations_and_methods(
        bubble,
        NamespaceId::from_raw(0),
        Vec::new(),
        vec![declaration],
        vec![hir],
        Vec::new(),
    )
    .expect("Bubble");
    assert!(bubble_hir.verify(resolver.arena()).is_ok());
    let dump = bubble_hir.dump(resolver.arena());
    assert!(dump.contains("match s"));
    assert!(dump.contains("when case#"));
}

#[test]
fn nominal_interfaces_retain_declarations_slot_maps_upcasts_and_dispatch() {
    let module = ModuleId::from_raw(0);
    let bubble = BubbleId::from_raw(0);
    let source = SourceFile::new(
        FileId::from_raw(0),
        "src/interfaces.pop",
        "namespace Example\n\
         public interface Closeable\n\
             function close()\n\
         end\n\
         public interface Reader\n\
             function read(count: Int): String\n\
         end\n\
         public class FileReader implements Reader\n\
             public function FileReader:read(count: Int): String\n\
                 return \"value\"\n\
             end\n\
         end\n\
         public function read(reader: FileReader): String\n\
             local contract: Reader = reader\n\
             return contract:read(1)\n\
         end\n",
    )
    .expect("source");
    let syntax = parse_file(&source);
    let interface_nodes: Vec<_> = syntax
        .root()
        .children()
        .iter()
        .filter(|node| node.kind() == NodeKind::InterfaceDeclaration)
        .collect();
    let class_node = node(&syntax, NodeKind::ClassDeclaration);
    let function_node = node(&syntax, NodeKind::FunctionDeclaration);
    let interface_syntax =
        parse_interface_declaration(&source, &syntax, interface_nodes[1]).expect("interface");
    let closeable_syntax =
        parse_interface_declaration(&source, &syntax, interface_nodes[0]).expect("interface");
    let class_syntax = parse_class_declaration(&source, &syntax, class_node).expect("class");
    let function_syntax =
        parse_function_signature(&source, &syntax, function_node).expect("signature");
    let function_body =
        parse_function_body(&source, &syntax, function_node, &function_syntax).expect("body");
    let indexed = build_declaration_index(&[ModuleInput::new(module, bubble, &source, &syntax)]);
    let interface_symbol = indexed
        .index()
        .declaration_by_qualified_name("Example.Reader", SymbolSpace::Type)[0]
        .symbol();
    let closeable_symbol = indexed
        .index()
        .declaration_by_qualified_name("Example.Closeable", SymbolSpace::Type)[0]
        .symbol();
    let class_symbol = indexed
        .index()
        .declaration_by_qualified_name("Example.FileReader", SymbolSpace::Type)[0]
        .symbol();
    let function_symbol = indexed
        .index()
        .declaration_by_qualified_name("Example.read", SymbolSpace::Value)[0]
        .symbol();
    let database = ResolutionDatabase::new(indexed.into_index());
    let mut resolver =
        SignatureResolver::new(&database, embedded_bootstrap_schema().expect("bootstrap"));
    let closeable = resolver
        .define_interface(module, closeable_symbol, &closeable_syntax)
        .definition()
        .expect("interface definition")
        .clone();
    let interface = resolver
        .define_interface(module, interface_symbol, &interface_syntax)
        .definition()
        .expect("interface definition")
        .clone();
    let class = resolver
        .define_class(module, class_symbol, &class_syntax)
        .definition()
        .expect("class definition")
        .clone();
    let function_signature = resolver
        .resolve(module, function_symbol, &function_syntax)
        .signature()
        .expect("function signature")
        .clone();
    let function_signatures = BTreeMap::from([(function_symbol, function_signature.clone())]);
    let typed_function = BodyChecker::new(module, &mut resolver, &function_signatures)
        .check(&function_signature, &function_body);
    assert!(
        typed_function.diagnostics().is_empty(),
        "{}",
        typed_function.diagnostic_snapshot()
    );
    let method = &class.methods()[0];
    let method_signature = resolver.method_signature(&class, method);
    let method_body =
        parse_class_method_body(&source, &syntax, class_node, &class_syntax.methods()[0])
            .expect("method body");
    let typed_method = BodyChecker::new(module, &mut resolver, &BTreeMap::new())
        .check(&method_signature, &method_body);
    assert!(
        typed_method.diagnostics().is_empty(),
        "{}",
        typed_method.diagnostic_snapshot()
    );
    let known_functions = BTreeSet::from([function_symbol]);
    let known_methods = BTreeSet::from([method.method()]);
    let interface_definitions = vec![closeable.clone(), interface.clone()];
    let known = HirKnownCallables::new(&known_functions, &known_methods)
        .with_interfaces(&interface_definitions);
    let hir_function = build_hir_function_with_known_callables_and_attributes(
        HirFunctionContext::new(module, bubble, Visibility::Public),
        &function_signature,
        typed_function.body().expect("typed function"),
        resolver.arena(),
        known,
        &[],
    )
    .expect("function HIR");
    let hir_method = build_hir_method(
        HirFunctionContext::new(module, bubble, Visibility::Public),
        &class,
        method,
        &method_signature,
        typed_method.body().expect("typed method"),
        resolver.arena(),
        known,
    )
    .expect("method HIR");
    let declarations = vec![
        HirDeclaration::interface(module, bubble, Visibility::Public, "Closeable", &closeable),
        HirDeclaration::interface(module, bubble, Visibility::Public, "Reader", &interface),
        HirDeclaration::class(module, bubble, Visibility::Public, "FileReader", &class),
    ];
    let bubble_hir = HirBubble::new_with_declarations_and_methods(
        bubble,
        NamespaceId::from_raw(0),
        Vec::new(),
        declarations,
        vec![hir_function],
        vec![hir_method],
    )
    .expect("Bubble");

    let interface_hir = bubble_hir.declarations()[1]
        .as_interface()
        .expect("interface HIR");
    assert_eq!(interface_hir.interface(), interface.interface());
    assert_eq!(interface_hir.methods()[0].slot(), 0);
    let class_hir = bubble_hir.declarations()[2].as_class().expect("class HIR");
    assert_eq!(class_hir.interfaces().len(), 1);
    assert_eq!(class_hir.interfaces()[0].methods()[0].slot(), 0);
    assert!(bubble_hir.verify(resolver.arena()).is_ok());
    let dump = bubble_hir.dump(resolver.arena());
    assert!(dump.contains("interface Reader"));
    assert!(dump.contains("call.interface"));
    assert!(dump.contains("call.interface i1 im1 slot0"));
    assert!(dump.contains("convert.interface"));
    assert!(!dump.to_ascii_lowercase().contains("lookup name"));
}

#[test]
fn compile_time_attribute_queries_cannot_escape_into_runtime_hir() {
    let module = ModuleId::from_raw(0);
    let bubble = BubbleId::from_raw(0);
    let source = SourceFile::new(
        FileId::from_raw(0),
        "src/query.pop",
        "namespace Example\n\
         public attribute Serializable()\n\
         public record User\n\
             name: String\n\
         end\n\
         public function inspect(): Boolean\n\
             return hasAttribute<<Serializable>>(User)\n\
         end\n",
    )
    .expect("source");
    let syntax = parse_file(&source);
    let attribute_node = node(&syntax, NodeKind::AttributeDeclaration);
    let record_node = node(&syntax, NodeKind::RecordDeclaration);
    let function_node = node(&syntax, NodeKind::FunctionDeclaration);
    let attribute_syntax =
        parse_attribute_declaration(&source, &syntax, attribute_node).expect("attribute");
    let record_syntax = parse_record_declaration(&source, &syntax, record_node).expect("record");
    let function_syntax =
        parse_function_signature(&source, &syntax, function_node).expect("function");
    let body_syntax =
        parse_function_body(&source, &syntax, function_node, &function_syntax).expect("body");
    let indexed = build_declaration_index(&[ModuleInput::new(module, bubble, &source, &syntax)]);
    let attribute_symbol = indexed
        .index()
        .declaration_by_qualified_name("Example.Serializable", SymbolSpace::Type)[0]
        .symbol();
    let record_symbol = indexed
        .index()
        .declaration_by_qualified_name("Example.User", SymbolSpace::Type)[0]
        .symbol();
    let function_symbol = indexed
        .index()
        .declaration_by_qualified_name("Example.inspect", SymbolSpace::Value)[0]
        .symbol();
    let database = ResolutionDatabase::new(indexed.into_index());
    let mut resolver =
        SignatureResolver::new(&database, embedded_bootstrap_schema().expect("bootstrap"));
    assert!(
        resolver
            .define_attribute(module, attribute_symbol, &attribute_syntax)
            .diagnostics()
            .is_empty()
    );
    assert!(
        resolver
            .define_record(module, record_symbol, &record_syntax)
            .diagnostics()
            .is_empty()
    );
    let signature = resolver
        .resolve(module, function_symbol, &function_syntax)
        .signature()
        .expect("signature")
        .clone();
    let signatures = BTreeMap::from([(function_symbol, signature.clone())]);
    let typed =
        BodyChecker::new(module, &mut resolver, &signatures).check(&signature, &body_syntax);
    assert!(
        typed.diagnostics().is_empty(),
        "{}",
        typed.diagnostic_snapshot()
    );
    let error = build_hir_function(
        module,
        bubble,
        Visibility::Public,
        &signature,
        typed.body().expect("typed query body"),
        resolver.arena(),
        &BTreeSet::from([function_symbol]),
    )
    .expect_err("compile-time compiler handles cannot enter runtime HIR");
    assert!(matches!(
        error.as_slice(),
        [HirVerificationError::CompileTimeOnlyExpression { .. }]
    ));
}
