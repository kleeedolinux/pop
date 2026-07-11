use pop_foundation::{BubbleId, FileId, ModuleId, SourceSpan, TextRange, TextSize};
use pop_resolve::{ModuleInput, ResolutionDatabase, SymbolSpace, build_declaration_index};
use pop_source::SourceFile;
use pop_syntax::parse_file;

fn use_span(file: FileId) -> SourceSpan {
    SourceSpan::new(
        file,
        TextRange::new(TextSize::from_u32(0), TextSize::from_u32(1)).expect("range"),
    )
}

#[test]
fn multi_module_resolution_obeys_using_and_visibility_boundaries() {
    let models = SourceFile::new(
        FileId::from_raw(0),
        "src/models.pop",
        "namespace Game.Models\n\
         public record Player\nend\n\
         internal record Cache\nend\n\
         private const SECRET = 1\n",
    )
    .expect("models");
    let same_bubble = SourceFile::new(
        FileId::from_raw(1),
        "src/service.pop",
        "namespace Game.Service\nusing Game.Models\npublic function run()\nend\n",
    )
    .expect("service");
    let other_bubble = SourceFile::new(
        FileId::from_raw(2),
        "src/tool.pop",
        "namespace Game.Tool\nusing Models = Game.Models\npublic function run()\nend\n",
    )
    .expect("tool");
    let model_syntax = parse_file(&models);
    let service_syntax = parse_file(&same_bubble);
    let tool_syntax = parse_file(&other_bubble);
    let indexed = build_declaration_index(&[
        ModuleInput::new(
            ModuleId::from_raw(0),
            BubbleId::from_raw(0),
            &models,
            &model_syntax,
        ),
        ModuleInput::new(
            ModuleId::from_raw(1),
            BubbleId::from_raw(0),
            &same_bubble,
            &service_syntax,
        ),
        ModuleInput::new(
            ModuleId::from_raw(2),
            BubbleId::from_raw(1),
            &other_bubble,
            &tool_syntax,
        ),
    ]);
    assert!(indexed.diagnostics().is_empty());
    let resolver = ResolutionDatabase::new(indexed.into_index());

    let player = resolver.resolve(
        ModuleId::from_raw(1),
        "Player",
        SymbolSpace::Type,
        use_span(FileId::from_raw(1)),
    );
    let cache = resolver.resolve(
        ModuleId::from_raw(1),
        "Cache",
        SymbolSpace::Type,
        use_span(FileId::from_raw(1)),
    );
    let private = resolver.resolve(
        ModuleId::from_raw(1),
        "SECRET",
        SymbolSpace::Value,
        use_span(FileId::from_raw(1)),
    );
    let aliased_player = resolver.resolve(
        ModuleId::from_raw(2),
        "Models.Player",
        SymbolSpace::Type,
        use_span(FileId::from_raw(2)),
    );
    let inaccessible_cache = resolver.resolve(
        ModuleId::from_raw(2),
        "Models.Cache",
        SymbolSpace::Type,
        use_span(FileId::from_raw(2)),
    );

    assert_eq!(player.symbol().expect("public Player").raw(), 0);
    assert_eq!(cache.symbol().expect("same-Bubble Cache").raw(), 1);
    assert_eq!(private.diagnostic_snapshot(), "POP1004@0..1\n");
    assert_eq!(aliased_player.symbol().expect("aliased Player").raw(), 0);
    assert_eq!(inaccessible_cache.diagnostic_snapshot(), "POP1004@0..1\n");
}

#[test]
fn ambiguous_imported_names_are_not_selected_by_order() {
    let first = SourceFile::new(
        FileId::from_raw(0),
        "src/a.pop",
        "namespace A\npublic record Item\nend\n",
    )
    .expect("a");
    let second = SourceFile::new(
        FileId::from_raw(1),
        "src/b.pop",
        "namespace B\npublic record Item\nend\n",
    )
    .expect("b");
    let consumer = SourceFile::new(
        FileId::from_raw(2),
        "src/c.pop",
        "namespace C\nusing A\nusing B\npublic function run()\nend\n",
    )
    .expect("c");
    let first_syntax = parse_file(&first);
    let second_syntax = parse_file(&second);
    let consumer_syntax = parse_file(&consumer);
    let indexed = build_declaration_index(&[
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
        ModuleInput::new(
            ModuleId::from_raw(2),
            BubbleId::from_raw(0),
            &consumer,
            &consumer_syntax,
        ),
    ]);
    let resolver = ResolutionDatabase::new(indexed.into_index());
    let resolution = resolver.resolve(
        ModuleId::from_raw(2),
        "Item",
        SymbolSpace::Type,
        use_span(FileId::from_raw(2)),
    );

    assert!(resolution.symbol().is_none());
    assert_eq!(resolution.diagnostic_snapshot(), "POP1003@0..1\n");
}

#[test]
fn unknown_names_never_become_dynamic_lookup() {
    let source = SourceFile::new(
        FileId::from_raw(0),
        "src/main.pop",
        "namespace Main\npublic function run()\nend\n",
    )
    .expect("source");
    let syntax = parse_file(&source);
    let indexed = build_declaration_index(&[ModuleInput::new(
        ModuleId::from_raw(0),
        BubbleId::from_raw(0),
        &source,
        &syntax,
    )]);
    let resolver = ResolutionDatabase::new(indexed.into_index());
    let resolution = resolver.resolve(
        ModuleId::from_raw(0),
        "Missing",
        SymbolSpace::Value,
        use_span(FileId::from_raw(0)),
    );

    assert!(resolution.symbol().is_none());
    assert_eq!(resolution.diagnostic_snapshot(), "POP1002@0..1\n");
}
