use pop_foundation::FileId;
use pop_source::SourceFile;
use pop_syntax::{
    ClassMethodDispatchSyntax, ExpressionSyntaxKind, NodeKind, StatementSyntaxKind,
    VisibilitySyntax, parse_class_declaration, parse_class_method_body, parse_file,
};

fn parse_class(text: &str) -> pop_syntax::ClassDeclarationSyntax {
    let source = SourceFile::new(FileId::from_raw(0), "src/connection.pop", text).expect("source");
    let syntax = parse_file(&source);
    assert!(syntax.diagnostics().is_empty(), "structural syntax");
    let class = syntax
        .root()
        .children()
        .iter()
        .find(|node| node.kind() == NodeKind::ClassDeclaration)
        .expect("class");
    parse_class_declaration(&source, &syntax, class).expect("typed class syntax")
}

#[test]
fn parses_native_class_fields_and_owned_method_signatures() {
    let class = parse_class(
        "namespace Network.Transport\n\
         public open class Connection\n\
             public endpoint: Network.Endpoint\n\
             private closed: Boolean = false\n\
             public function Connection.new(endpoint: Network.Endpoint): Connection\n\
                 return Connection { endpoint = endpoint }\n\
             end\n\
             public function Connection:close()\n\
                 if not self.closed then\n\
                     self.closed = true\n\
                 end\n\
             end\n\
         end\n",
    );

    assert_eq!(class.name(), "Connection");
    assert!(class.is_open());
    assert_eq!(class.fields().len(), 2);
    assert_eq!(class.fields()[0].visibility(), VisibilitySyntax::Public);
    assert_eq!(class.fields()[0].name(), "endpoint");
    assert!(class.fields()[0].default().is_none());
    assert_eq!(class.fields()[1].visibility(), VisibilitySyntax::Private);
    assert!(matches!(
        class.fields()[1]
            .default()
            .map(pop_syntax::ExpressionSyntax::kind),
        Some(ExpressionSyntaxKind::Boolean(false))
    ));

    assert_eq!(class.methods().len(), 2);
    assert_eq!(class.methods()[0].owner(), "Connection");
    assert_eq!(class.methods()[0].name(), "new");
    assert_eq!(
        class.methods()[0].dispatch(),
        ClassMethodDispatchSyntax::Static
    );
    assert_eq!(class.methods()[0].parameters().len(), 1);
    assert_eq!(class.methods()[0].results().len(), 1);
    assert_eq!(
        class.methods()[1].dispatch(),
        ClassMethodDispatchSyntax::Receiver
    );
    assert_eq!(class.methods()[1].name(), "close");
    assert!(class.methods()[1].parameters().is_empty());
}

#[test]
fn class_members_require_explicit_visibility() {
    let source = SourceFile::new(
        FileId::from_raw(0),
        "src/invalid.pop",
        "namespace Example\n\
         public class Connection\n\
             closed: Boolean\n\
         end\n",
    )
    .expect("source");
    let syntax = parse_file(&source);
    let class = syntax
        .root()
        .children()
        .iter()
        .find(|node| node.kind() == NodeKind::ClassDeclaration)
        .expect("class");
    let error = parse_class_declaration(&source, &syntax, class)
        .expect_err("class member visibility is mandatory");

    assert_eq!(error.expectation(), "class member visibility");
}

#[test]
fn parses_receiver_and_static_method_bodies_with_their_exact_boundaries() {
    let source = SourceFile::new(
        FileId::from_raw(0),
        "src/counter.pop",
        "namespace Example\n\
         public class Counter\n\
             public value: Int\n\
             public function Counter.new(value: Int): Counter\n\
                 return Counter { value = value }\n\
             end\n\
             public function Counter:get(): Int\n\
                 return self.value\n\
             end\n\
         end\n",
    )
    .expect("source");
    let syntax = parse_file(&source);
    let class_node = syntax
        .root()
        .children()
        .iter()
        .find(|node| node.kind() == NodeKind::ClassDeclaration)
        .expect("class");
    let class = parse_class_declaration(&source, &syntax, class_node).expect("class syntax");
    let static_body = parse_class_method_body(&source, &syntax, class_node, &class.methods()[0])
        .expect("static body");
    let receiver_body = parse_class_method_body(&source, &syntax, class_node, &class.methods()[1])
        .expect("receiver body");

    let StatementSyntaxKind::Return { values } = static_body.statements()[0].kind() else {
        panic!("static return");
    };
    assert!(matches!(
        values[0].kind(),
        ExpressionSyntaxKind::Construct { type_name, .. } if type_name == &["Counter"]
    ));
    let StatementSyntaxKind::Return { values } = receiver_body.statements()[0].kind() else {
        panic!("receiver return");
    };
    assert!(matches!(
        values[0].kind(),
        ExpressionSyntaxKind::Name(path) if path == &["self", "value"]
    ));
}
