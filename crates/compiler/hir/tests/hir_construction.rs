use std::collections::{BTreeMap, BTreeSet};

use pop_foundation::{BubbleId, FileId, ModuleId, NamespaceId, SymbolId};
use pop_hir::{
    HirBubble, HirCallDispatch, HirExpressionKind, HirFunctionContext, HirStatementKind,
    build_hir_function, build_hir_function_with_attributes, verify_hir_function,
};
use pop_resolve::{
    ModuleInput, ResolutionDatabase, SymbolSpace, Visibility, build_declaration_index,
};
use pop_source::SourceFile;
use pop_syntax::{
    NodeKind, parse_attribute_declaration, parse_attribute_use, parse_file, parse_function_body,
    parse_function_signature, parse_record_declaration, parse_union_declaration,
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
             local firstName: String? = names[1]\n\
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
            if matches!(elements[0].kind(), HirExpressionKind::String(value) if value == "\"first\"")
                && matches!(elements[1].kind(), HirExpressionKind::String(value) if value == "\"second\"")
    ));
    let HirStatementKind::Local { initializer, .. } = hir.body()[1].kind() else {
        panic!("table local");
    };
    assert!(matches!(
        initializer.kind(),
        HirExpressionKind::Table(entries)
            if matches!(entries[0].key().kind(), HirExpressionKind::String(value) if value == "\"first\"")
                && matches!(entries[1].value().kind(), HirExpressionKind::Integer(value) if value.to_string() == "2")
    ));
    let HirStatementKind::Local { initializer, .. } = hir.body()[2].kind() else {
        panic!("indexed local");
    };
    assert!(matches!(
        initializer.kind(),
        HirExpressionKind::ArrayGet { array, index }
            if matches!(array.kind(), HirExpressionKind::Local(_))
                && matches!(index.kind(), HirExpressionKind::Integer(value) if value.to_string() == "1")
    ));
    assert!(verify_hir_function(&hir, resolver.arena(), &known).is_ok());
}
