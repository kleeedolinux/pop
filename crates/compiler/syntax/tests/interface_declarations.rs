use pop_foundation::FileId;
use pop_source::SourceFile;
use pop_syntax::{
    NodeKind, TypeSyntaxKind, parse_class_declaration, parse_file, parse_interface_declaration,
};

#[test]
fn parses_public_instance_interface_signatures_and_explicit_class_implementation() {
    let source = SourceFile::new(
        FileId::from_raw(0),
        "src/reader.pop",
        "namespace Storage\n\
         public interface Reader\n\
             function read(count: Int): String\n\
             function close()\n\
         end\n\
         public class FileReader implements Reader, Resource.Closeable\n\
             public function FileReader:read(count: Int): String\n\
                 return \"\"\n\
             end\n\
             public function FileReader:close()\n\
             end\n\
         end\n",
    )
    .expect("source");
    let syntax = parse_file(&source);
    assert!(syntax.diagnostics().is_empty(), "structural syntax");

    let interface_node = syntax
        .root()
        .children()
        .iter()
        .find(|node| node.kind() == NodeKind::InterfaceDeclaration)
        .expect("interface");
    let interface = parse_interface_declaration(&source, &syntax, interface_node)
        .expect("typed interface syntax");
    assert_eq!(interface.name(), "Reader");
    assert_eq!(interface.methods().len(), 2);
    assert_eq!(interface.methods()[0].name(), "read");
    assert_eq!(interface.methods()[0].parameters().len(), 1);
    assert_eq!(interface.methods()[0].results().len(), 1);
    assert_eq!(interface.methods()[1].name(), "close");
    assert!(interface.methods()[1].results().is_empty());

    let class_node = syntax
        .root()
        .children()
        .iter()
        .find(|node| node.kind() == NodeKind::ClassDeclaration)
        .expect("class");
    let class = parse_class_declaration(&source, &syntax, class_node).expect("class syntax");
    assert_eq!(class.interfaces().len(), 2);
    assert!(matches!(
        class.interfaces()[0].kind(),
        TypeSyntaxKind::Named { path, .. } if path.as_slice() == ["Reader"]
    ));
    assert!(matches!(
        class.interfaces()[1].kind(),
        TypeSyntaxKind::Named { path, .. } if path.as_slice() == ["Resource", "Closeable"]
    ));
}

#[test]
fn generic_classes_and_interfaces_preserve_ordered_parameters_and_bounds() {
    let source = SourceFile::new(
        FileId::from_raw(0),
        "src/genericIterator.pop",
        "namespace Iteration\n\
         public interface Producer<T>\n\
             function next(): T?\n\
         end\n\
         private class MappingIterator<T, TSource: Producer<T>> implements Producer<T>\n\
             source: TSource\n\
             public function MappingIterator:next(): T?\n\
                 return nil\n\
             end\n\
         end\n",
    )
    .expect("source");
    let syntax = parse_file(&source);
    assert!(syntax.diagnostics().is_empty(), "structural syntax");

    let interface_node = syntax
        .root()
        .children()
        .iter()
        .find(|node| node.kind() == NodeKind::InterfaceDeclaration)
        .expect("interface");
    let interface = parse_interface_declaration(&source, &syntax, interface_node)
        .expect("generic interface syntax");
    assert_eq!(interface.type_parameters().len(), 1);
    assert_eq!(interface.type_parameters()[0].name(), "T");

    let class_node = syntax
        .root()
        .children()
        .iter()
        .find(|node| node.kind() == NodeKind::ClassDeclaration)
        .expect("class");
    let class =
        parse_class_declaration(&source, &syntax, class_node).expect("generic class syntax");
    assert_eq!(
        class
            .type_parameters()
            .iter()
            .map(pop_syntax::GenericParameterSyntax::name)
            .collect::<Vec<_>>(),
        ["T", "TSource"]
    );
    assert!(class.type_parameters()[0].bound().is_none());
    assert!(matches!(
        class.type_parameters()[1]
            .bound()
            .map(pop_syntax::TypeSyntax::kind),
        Some(TypeSyntaxKind::Named { path, arguments })
            if path == &["Producer"] && arguments.len() == 1
    ));
    assert!(matches!(
        class.interfaces()[0].kind(),
        TypeSyntaxKind::Named { path, arguments }
            if path == &["Producer"] && arguments.len() == 1
    ));
}

#[test]
fn interface_members_reject_visibility_fields_and_bodies() {
    for invalid in [
        "namespace Example\npublic interface Reader\npublic function read()\nend\n",
        "namespace Example\npublic interface Reader\nvalue: Int\nend\n",
        "namespace Example\npublic interface Reader\nfunction read()\nreturn\nend\nend\n",
    ] {
        let source =
            SourceFile::new(FileId::from_raw(0), "src/invalid.pop", invalid).expect("source");
        let syntax = parse_file(&source);
        let node = syntax
            .root()
            .children()
            .iter()
            .find(|node| node.kind() == NodeKind::InterfaceDeclaration)
            .expect("interface");
        assert!(
            parse_interface_declaration(&source, &syntax, node).is_err(),
            "invalid interface member must not gain semantics"
        );
    }
}
