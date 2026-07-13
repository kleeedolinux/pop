use pop_foundation::{BubbleId, FileId, ModuleId};
use pop_resolve::{DeclarationKind, ModuleInput, SymbolSpace, Visibility, build_declaration_index};
use pop_source::SourceFile;
use pop_syntax::parse_file;

#[test]
fn declaration_index_is_deterministic_when_inputs_arrive_reversed() {
    let first = SourceFile::new(
        FileId::from_raw(0),
        "src/models.pop",
        "namespace Game.Models\npublic record Player\nend\nprivate const SECRET = 1\n",
    )
    .expect("source");
    let second = SourceFile::new(
        FileId::from_raw(1),
        "src/service.pop",
        "namespace Game.Service\nusing Game.Models\npublic function loadPlayer(): Player\nend\n",
    )
    .expect("source");
    let first_syntax = parse_file(&first);
    let second_syntax = parse_file(&second);

    let forward = build_declaration_index(&[
        ModuleInput::new(
            ModuleId::from_raw(0),
            BubbleId::from_raw(0),
            &first,
            &first_syntax,
        ),
        ModuleInput::new(
            ModuleId::from_raw(1),
            BubbleId::from_raw(0),
            &second,
            &second_syntax,
        ),
    ]);
    let reverse = build_declaration_index(&[
        ModuleInput::new(
            ModuleId::from_raw(1),
            BubbleId::from_raw(0),
            &second,
            &second_syntax,
        ),
        ModuleInput::new(
            ModuleId::from_raw(0),
            BubbleId::from_raw(0),
            &first,
            &first_syntax,
        ),
    ]);

    assert!(forward.diagnostics().is_empty());
    assert!(reverse.diagnostics().is_empty());
    assert_eq!(forward.index().dump(), reverse.index().dump());
    assert_eq!(
        forward.index().dump(),
        "module 0 bubble 0 namespace Game.Models\n\
         symbol 0 public type Record Game.Models.Player\n\
         symbol 1 private value Constant Game.Models.SECRET\n\
         module 1 bubble 0 namespace Game.Service\n\
         using Game.Models\n\
         symbol 2 public value Function Game.Service.loadPlayer\n"
    );

    let player = forward
        .index()
        .declaration_by_qualified_name("Game.Models.Player", SymbolSpace::Type);
    assert_eq!(player.len(), 1);
    assert_eq!(player[0].symbol().raw(), 0);
}

#[test]
fn using_aliases_are_indexed_without_creating_runtime_operations() {
    let source = SourceFile::new(
        FileId::from_raw(0),
        "src/main.pop",
        "namespace Game.Main\nusing Models = Game.Models\npublic function run()\nend\n",
    )
    .expect("source");
    let syntax = parse_file(&source);
    let result = build_declaration_index(&[ModuleInput::new(
        ModuleId::from_raw(0),
        BubbleId::from_raw(0),
        &source,
        &syntax,
    )]);
    let module = result
        .index()
        .module(ModuleId::from_raw(0))
        .expect("module");

    assert!(result.diagnostics().is_empty());
    assert_eq!(module.usings()[0].alias(), Some("Models"));
    assert_eq!(module.usings()[0].namespace(), "Game.Models");
}

#[test]
fn omitted_visibility_defaults_to_internal_except_for_binary_main() {
    let ordinary = SourceFile::new(
        FileId::from_raw(0),
        "src/ordinary.pop",
        "namespace Example\n\
         function run()\n\
         end\n\
         const VALUE = 1\n\
         type Name = String\n\
         attribute Marker()\n\
         record Data\n\
         end\n\
         union Choice\n\
         end\n\
         error Failure\n\
             Invalid\n\
         end\n\
         class Service\n\
         end\n\
         interface Reader\n\
         end\n\
         enum Color\n\
         end\n",
    )
    .expect("source");
    let ordinary_syntax = parse_file(&ordinary);
    let ordinary_index = build_declaration_index(&[ModuleInput::new(
        ModuleId::from_raw(0),
        BubbleId::from_raw(0),
        &ordinary,
        &ordinary_syntax,
    )]);
    assert!(ordinary_index.diagnostics().is_empty());
    for declaration in ordinary_index.index().declarations() {
        assert_eq!(declaration.visibility(), Visibility::Internal);
        assert!(!declaration.is_in_public_reference_surface());
    }

    let library_main = SourceFile::new(
        FileId::from_raw(2),
        "src/library.pop",
        "namespace Library\nfunction main()\nend\n",
    )
    .expect("source");
    let library_main_syntax = parse_file(&library_main);
    let library_main_index = build_declaration_index(&[ModuleInput::new(
        ModuleId::from_raw(2),
        BubbleId::from_raw(0),
        &library_main,
        &library_main_syntax,
    )]);
    assert_eq!(
        library_main_index
            .index()
            .declarations()
            .next()
            .expect("library main declaration")
            .visibility(),
        Visibility::Internal
    );

    let entry = SourceFile::new(
        FileId::from_raw(1),
        "src/main.pop",
        "namespace Application\nfunction main()\nend\n",
    )
    .expect("source");
    let entry_syntax = parse_file(&entry);
    let entry_index = build_declaration_index(&[ModuleInput::new(
        ModuleId::from_raw(1),
        BubbleId::from_raw(0),
        &entry,
        &entry_syntax,
    )
    .with_implicit_main_entry()]);
    assert!(entry_index.diagnostics().is_empty());
    assert_eq!(
        entry_index
            .index()
            .declarations()
            .next()
            .expect("entry declaration")
            .visibility(),
        Visibility::Private
    );
}

#[test]
fn contextual_error_declarations_have_a_distinct_type_identity() {
    let source = SourceFile::new(
        FileId::from_raw(0),
        "src/errors.pop",
        "namespace Example\npublic error LoadError\n    Missing\nend\n",
    )
    .expect("source");
    let syntax = parse_file(&source);
    let result = build_declaration_index(&[ModuleInput::new(
        ModuleId::from_raw(0),
        BubbleId::from_raw(0),
        &source,
        &syntax,
    )]);

    assert!(result.diagnostics().is_empty());
    let errors = result
        .index()
        .declaration_by_qualified_name("Example.LoadError", SymbolSpace::Type);
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].kind(), DeclarationKind::Error);
    assert_eq!(errors[0].visibility(), Visibility::Public);
}
