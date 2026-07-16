use std::path::PathBuf;

use pop_localization::{Language, LanguageSources, select_language};

#[test]
fn supported_tags_and_documented_aliases_normalize_deterministically() {
    let cases = [
        ("en_US.UTF-8", Language::English),
        ("zh", Language::SimplifiedChinese),
        ("zh_CN.UTF-8", Language::SimplifiedChinese),
        ("ja-JP", Language::Japanese),
        ("pt", Language::PortugueseBrazil),
        ("pt_BR.UTF-8", Language::PortugueseBrazil),
        ("es-MX", Language::Spanish),
    ];
    for (tag, expected) in cases {
        assert_eq!(Language::from_tag(tag), Some(expected), "{tag}");
    }
    assert_eq!(Language::from_tag("fr-FR"), None);
}

#[test]
fn explicit_environment_config_system_and_english_follow_precedence() {
    let sources = LanguageSources {
        explicit: Some("ja".to_owned()),
        environment: Some("pt-BR".to_owned()),
        config: Some("es".to_owned()),
        system: Some("zh-Hans".to_owned()),
        config_path: None,
    };
    assert_eq!(
        select_language(&sources).expect("selection"),
        Language::Japanese
    );

    let without_explicit = LanguageSources {
        explicit: None,
        ..sources.clone()
    };
    assert_eq!(
        select_language(&without_explicit).unwrap(),
        Language::PortugueseBrazil
    );
    let config = LanguageSources {
        environment: None,
        ..without_explicit.clone()
    };
    assert_eq!(select_language(&config).unwrap(), Language::Spanish);
    let system = LanguageSources {
        config: None,
        ..config.clone()
    };
    assert_eq!(
        select_language(&system).unwrap(),
        Language::SimplifiedChinese
    );
    let default = LanguageSources {
        system: None,
        ..system
    };
    assert_eq!(select_language(&default).unwrap(), Language::English);
}

#[test]
fn an_unsupported_explicit_language_is_an_error() {
    let sources = LanguageSources {
        explicit: Some("fr".to_owned()),
        environment: None,
        config: None,
        system: None,
        config_path: Some(PathBuf::from("/tmp/config.toml")),
    };
    let error = select_language(&sources).expect_err("explicit unsupported tag");
    assert!(error.to_string().contains("fr"));
}
