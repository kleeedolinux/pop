use pop_foundation::FileId;
use pop_source::SourceFile;
use pop_syntax::{NodeKind, parse_file};

fn parse(text: &str) -> pop_syntax::SyntaxTree {
    let source =
        SourceFile::new(FileId::from_raw(0), "src/shapes.pop", text).expect("small source");
    parse_file(&source)
}

#[test]
fn canonical_data_and_declaration_file_has_stable_structure() {
    let tree = parse(
        "namespace Game.Data\n\
         \n\
         using Pop.Text\n\
         \n\
         public attribute Serializable(version: UInt32 = 1)\n\
         \n\
         private const INITIAL_SCORE = 0\n\
         internal type Predicate<T> = function(value: T): Boolean\n\
         \n\
         @Serializable(version = 1)\n\
         public record Player\n\
             name: String\n\
             score: Int = INITIAL_SCORE\n\
         end\n\
         \n\
         public union LoadState\n\
             Idle\n\
             Ready(player: Player)\n\
         end\n",
    );

    assert!(
        tree.diagnostics().is_empty(),
        "{}",
        tree.diagnostic_snapshot()
    );
    assert_eq!(
        tree.root()
            .children()
            .iter()
            .map(pop_syntax::SyntaxNode::kind)
            .collect::<Vec<_>>(),
        [
            NodeKind::NamespaceDeclaration,
            NodeKind::UsingDirective,
            NodeKind::AttributeDeclaration,
            NodeKind::ConstDeclaration,
            NodeKind::TypeAliasDeclaration,
            NodeKind::AttributeUse,
            NodeKind::RecordDeclaration,
            NodeKind::UnionDeclaration,
        ]
    );
}

#[test]
fn class_blocks_include_nested_methods_and_control_flow() {
    let tree = parse(
        "namespace Network.Transport\n\
         \n\
         public open class Connection\n\
             private closed: Boolean = false\n\
         \n\
             public function Connection:close()\n\
                 if not self.closed then\n\
                     self.closed = true\n\
                 end\n\
             end\n\
         end\n",
    );

    assert!(
        tree.diagnostics().is_empty(),
        "{}",
        tree.diagnostic_snapshot()
    );
    assert_eq!(tree.root().children().len(), 2);
    assert_eq!(tree.root().children()[1].kind(), NodeKind::ClassDeclaration);
}

#[test]
fn every_namespace_declaration_kind_requires_visibility() {
    let tree = parse(
        "namespace Broken\n\
         const VALUE = 1\n\
         type Name = String\n\
         attribute Marker()\n\
         record Data\n\
         end\n\
         union Choice\n\
         end\n\
         class Service\n\
         end\n\
         interface Reader\n\
         end\n\
         enum Color\n\
         end\n",
    );
    let codes: Vec<_> = tree
        .diagnostics()
        .iter()
        .map(|diagnostic| diagnostic.code().as_str())
        .collect();

    assert_eq!(codes, ["POP0005"; 8]);
}

#[test]
fn export_recovery_applies_to_native_data_declarations() {
    let tree = parse("namespace Broken\nexport record Data\nend\n");

    assert_eq!(tree.diagnostic_snapshot(), "POP0004@17..23\n");
    assert_eq!(
        tree.root().children()[1].kind(),
        NodeKind::RecordDeclaration
    );
}

#[test]
fn semicolon_is_not_silently_accepted_as_declaration_punctuation() {
    let tree = parse("namespace Broken;\npublic const VALUE = 1;\n");

    assert_eq!(
        tree.diagnostic_snapshot(),
        "POP0001@16..17\nPOP0001@40..41\n"
    );
}
