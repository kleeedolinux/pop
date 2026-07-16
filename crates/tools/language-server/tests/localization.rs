use pop_diagnostics::resolution;
use pop_foundation::{FileId, SourceSpan, TextRange, TextSize};
use pop_language_server::LanguageServerSession;
use pop_localization::Language;

#[test]
fn language_server_sessions_select_locales_independently() {
    let english = LanguageServerSession::initialize(Some("en")).expect("English session");
    let japanese = LanguageServerSession::initialize(Some("ja-JP")).expect("Japanese session");
    assert_eq!(english.language(), Language::English);
    assert_eq!(japanese.language(), Language::Japanese);

    let diagnostic = resolution::unknown_name(
        SourceSpan::new(
            FileId::from_raw(1),
            TextRange::new(TextSize::from_u32(0), TextSize::from_u32(4)).expect("range"),
        ),
        "missingValue",
    );
    let en = english
        .render_diagnostic(&diagnostic)
        .expect("English rendering");
    let ja = japanese
        .render_diagnostic(&diagnostic)
        .expect("Japanese rendering");
    assert!(en.contains("POP1002"));
    assert!(ja.contains("POP1002"));
    assert!(en.contains("missingValue"));
    assert!(ja.contains("missingValue"));
    assert_ne!(en, ja);
}
