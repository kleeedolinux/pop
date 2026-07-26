use std::collections::BTreeMap;

use pop_foundation::{
    Diagnostic, DiagnosticArgument, DiagnosticCategory, DiagnosticCode, DiagnosticLabel,
    DiagnosticNote, DiagnosticSeverity, FileId, FixApplicability, MessageKey, QuickFix, SourceSpan,
    TextEdit, TextRange, TextSize, WorkspaceEdit,
};
use pop_localization::{Argument, DiagnosticSource, Language, RenderContext, official_catalogs};

fn repository_root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("localization crate is under repository root")
        .to_path_buf()
}

#[test]
fn catalogs_are_split_by_component_and_subdomain() {
    let root = repository_root().join("assets/locales");
    let fragments = [
        "cli/errors.toml",
        "cli/help.toml",
        "compiler/backend.toml",
        "compiler/compile-time.toml",
        "compiler/documentation.toml",
        "compiler/ffi.toml",
        "compiler/fixes.toml",
        "compiler/lexer.toml",
        "compiler/parser.toml",
        "compiler/resolution.toml",
        "compiler/types.toml",
        "lsp/session.toml",
        "shared/presentation.toml",
    ];
    for language in Language::ALL {
        let directory = root.join(language.tag());
        assert!(directory.join("manifest.toml").is_file());
        assert!(!root.join(format!("{}.toml", language.tag())).exists());
        for fragment in fragments {
            assert!(
                directory.join(fragment).is_file(),
                "{}: {fragment}",
                language.tag()
            );
        }
    }
}

#[test]
fn every_official_catalog_has_exact_key_argument_and_kind_parity() {
    let catalogs = official_catalogs().expect("all embedded catalogs are valid");
    assert_eq!(catalogs.len(), 5);

    let canonical = &catalogs[&Language::English];
    assert!(canonical.messages().len() >= 70);
    for language in Language::ALL {
        let catalog = &catalogs[&language];
        assert_eq!(catalog.tag(), language.tag());
        assert_eq!(
            catalog.messages().keys().collect::<Vec<_>>(),
            canonical.messages().keys().collect::<Vec<_>>()
        );
        for (key, message) in catalog.messages() {
            let expected = &canonical.messages()[key];
            assert_eq!(
                message.arguments(),
                expected.arguments(),
                "argument names differ for {key} in {}",
                language.tag()
            );
            assert_eq!(
                message.kinds(),
                expected.kinds(),
                "argument kinds differ for {key} in {}",
                language.tag()
            );
        }
    }
}

#[test]
fn every_registered_compiler_message_and_help_key_is_localized() {
    let catalog = official_catalogs().expect("all embedded catalogs are valid");
    let english = &catalog[&Language::English];
    let diagnostics =
        std::fs::read_to_string(repository_root().join("crates/compiler/diagnostics/catalog.tsv"))
            .expect("diagnostic catalog");

    for (line_number, line) in diagnostics.lines().enumerate().skip(1) {
        let columns = line.split('\t').collect::<Vec<_>>();
        assert_eq!(columns.len(), 13, "diagnostic row {}", line_number + 1);
        for column in [4, 5] {
            let key = columns[column];
            if key != "-" {
                assert!(
                    english.messages().contains_key(key),
                    "diagnostic {} references missing localization key {key}",
                    columns[0]
                );
            }
        }
    }
}

#[test]
fn all_catalog_messages_render_with_typed_placeholder_values() {
    let catalogs = official_catalogs().expect("catalogs");
    for language in Language::ALL {
        let context = RenderContext::new(language);
        for (key, message) in catalogs[&language].messages() {
            let arguments = message
                .arguments()
                .iter()
                .zip(message.kinds())
                .map(|(name, kind)| match kind.as_str() {
                    "Character" => Argument::character(name, 'x'),
                    "Unsigned" => Argument::unsigned(name, 2),
                    "External" => Argument::external(name, "external detail"),
                    _ => Argument::text(name, "Sample"),
                })
                .collect::<Vec<_>>();
            let rendered = context.message(key, &arguments).unwrap_or_else(|error| {
                panic!("{} failed to render {key}: {error}", language.tag())
            });
            assert!(!rendered.contains('{'), "unexpanded placeholder in {key}");
        }
    }
}

#[test]
fn interpolation_treats_argument_text_as_data() {
    let context = RenderContext::new(Language::English);
    let rendered = context
        .message(
            "resolution.unknownName",
            &[Argument::text("name", "{other}\u{1b}[31m")],
        )
        .expect("known message");
    assert!(rendered.contains("{other}"));
    assert!(!rendered.contains('\u{1b}'));
}

#[test]
fn diagnostic_message_labels_notes_and_fixes_share_the_locale() {
    let span = SourceSpan::new(
        FileId::from_raw(1),
        TextRange::new(TextSize::from_u32(4), TextSize::from_u32(8)).expect("range"),
    );
    let diagnostic = Diagnostic::new(
        DiagnosticCode::new("POP1002"),
        DiagnosticSeverity::Error,
        DiagnosticCategory::Resolution,
        MessageKey::new("resolution.unknownName"),
        vec![DiagnosticArgument::Identifier("missingValue".to_owned())],
        span,
    )
    .with_label(DiagnosticLabel::new(
        span,
        MessageKey::new("resolution.declaration"),
        Vec::new(),
    ))
    .with_note(DiagnosticNote::new(
        MessageKey::new("types.missingRecordField"),
        vec![DiagnosticArgument::Identifier("presentValue".to_owned())],
    ))
    .with_fix(QuickFix::new(
        "test.fix",
        MessageKey::new("fix.replaceExportWithPublic"),
        FixApplicability::Safe,
        WorkspaceEdit::new(
            0,
            vec![TextEdit::new(
                FileId::from_raw(1),
                TextRange::empty(TextSize::from_u32(0)),
                "",
            )],
        ),
    ));

    let mut outputs = BTreeMap::new();
    for language in Language::ALL {
        let output = RenderContext::new(language)
            .diagnostic(&diagnostic)
            .expect("diagnostic renders");
        assert!(output.contains("POP1002"));
        assert!(output.contains("missingValue"));
        assert!(output.contains("presentValue"));
        outputs.insert(language, output);
    }
    assert_eq!(outputs.len(), 5);
    assert_ne!(outputs[&Language::English], outputs[&Language::Japanese]);
    assert_ne!(
        outputs[&Language::English],
        outputs[&Language::PortugueseBrazil]
    );
}

#[test]
fn human_diagnostic_uses_real_source_path_line_column_excerpt_and_underline() {
    let source = "namespace Example\n    malformed value\n";
    let diagnostic = Diagnostic::new(
        DiagnosticCode::new("POP0002"),
        DiagnosticSeverity::Error,
        DiagnosticCategory::Syntax,
        MessageKey::new("syntax.unexpectedToken"),
        vec![
            DiagnosticArgument::SyntaxExpectation("end of statement"),
            DiagnosticArgument::Token("malformed declaration".to_owned()),
        ],
        SourceSpan::new(
            FileId::from_raw(7),
            TextRange::new(TextSize::from_u32(22), TextSize::from_u32(31)).expect("range"),
        ),
    );

    let rendered = RenderContext::new(Language::English)
        .diagnostic_with_sources(
            &diagnostic,
            &[DiagnosticSource::new(
                FileId::from_raw(7),
                "rust_les/nativeClass.pop",
                source,
            )],
        )
        .expect("source diagnostic");

    assert!(
        rendered.contains("--> rust_les/nativeClass.pop:2:5"),
        "{rendered}"
    );
    assert!(rendered.contains("2 |     malformed value"), "{rendered}");
    assert!(rendered.contains("|     ^^^^^^^^^"), "{rendered}");
    assert!(!rendered.contains("file#7"), "{rendered}");
    let source_line = rendered
        .lines()
        .find(|line| line.contains("malformed value"))
        .expect("source line");
    let caret_line = rendered
        .lines()
        .find(|line| line.contains("^^^^^^^^^"))
        .expect("caret line");
    assert_eq!(
        source_line.find("malformed").expect("source column"),
        caret_line.find('^').expect("caret column"),
        "caret must begin directly below the diagnosed text:\n{rendered}"
    );
    assert_eq!(source_line.find('|'), caret_line.find('|'), "{rendered}");
}

#[test]
fn human_diagnostic_aligns_tabs_and_wide_unicode_by_display_column() {
    fn terminal_width(text: &str) -> usize {
        text.chars()
            .map(|character| match character {
                '\u{0300}'..='\u{036f}' => 0,
                '界' => 2,
                _ => 1,
            })
            .sum()
    }

    let source = "\t界 malformed value\n";
    let diagnostic = Diagnostic::new(
        DiagnosticCode::new("POP0002"),
        DiagnosticSeverity::Error,
        DiagnosticCategory::Syntax,
        MessageKey::new("syntax.unexpectedToken"),
        vec![
            DiagnosticArgument::SyntaxExpectation("end of statement"),
            DiagnosticArgument::Token("malformed declaration".to_owned()),
        ],
        SourceSpan::new(
            FileId::from_raw(7),
            TextRange::new(TextSize::from_u32(5), TextSize::from_u32(14)).expect("range"),
        ),
    );

    let rendered = RenderContext::new(Language::English)
        .diagnostic_with_sources_and_width(
            &diagnostic,
            &[DiagnosticSource::new(
                FileId::from_raw(7),
                "rust_les/nativeClass.pop",
                source,
            )],
            terminal_width,
        )
        .expect("source diagnostic");

    assert!(
        rendered.contains("--> rust_les/nativeClass.pop:1:8"),
        "{rendered}"
    );
    assert!(!rendered.contains('\t'), "{rendered}");
    let source_line = rendered
        .lines()
        .find(|line| line.contains("malformed value"))
        .expect("source line");
    let caret_line = rendered
        .lines()
        .find(|line| line.contains("^^^^^^^^^"))
        .expect("caret line");
    let source_body = source_line.split_once("| ").expect("source gutter").1;
    let caret_body = caret_line.split_once("| ").expect("caret gutter").1;
    let source_prefix = source_body
        .split_once("malformed")
        .expect("diagnosed source")
        .0;
    let caret_prefix = caret_body.split_once('^').expect("caret").0;
    assert_eq!(
        terminal_width(source_prefix),
        terminal_width(caret_prefix),
        "caret must align by terminal display width:\n{rendered}"
    );
}
