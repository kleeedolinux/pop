use pop_foundation::{BubbleId, FileId, ModuleId};
use pop_resolve::{ModuleInput, SymbolSpace, build_declaration_index};
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
