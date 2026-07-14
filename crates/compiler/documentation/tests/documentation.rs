use pop_documentation::{
    DocumentationAnalysis, PublicErrorDocumentationContract, TypedErrorDocumentationContract,
    TypedReturnsDocumentationContract, XmlNode,
};
use pop_foundation::{DiagnosticCategory, DiagnosticSeverity, FileId};
use pop_source::SourceFile;
use pop_syntax::{NodeKind, parse_file};

fn analyze(text: &str) -> DocumentationAnalysis {
    let source = SourceFile::new(FileId::from_raw(0), "src/docs.pop", text).expect("small source");
    let syntax = parse_file(&source);
    assert!(
        syntax.diagnostics().is_empty(),
        "{}",
        syntax.diagnostic_snapshot()
    );
    DocumentationAnalysis::analyze(&source, &syntax)
}

#[test]
fn documentation_attaches_through_attributes_to_the_next_declaration() {
    let analysis = analyze(
        "namespace Saves\n\
         \n\
         --- <summary>\n\
         --- Represents a saved player.\n\
         --- </summary>\n\
         @Serializable(version = 1)\n\
         public record PlayerSave\n\
             name: String\n\
         end\n",
    );
    let block = &analysis.blocks()[0];
    let target = block.target().expect("attached declaration");

    assert!(analysis.diagnostics().is_empty());
    assert_eq!(target.kind(), NodeKind::RecordDeclaration);
    assert_eq!(
        block.xml_text(),
        "<summary>\nRepresents a saved player.\n</summary>"
    );
    let XmlNode::Element { name, children, .. } = &block.xml().expect("safe XML").children()[0]
    else {
        panic!("summary element");
    };
    assert_eq!(name, "summary");
    assert_eq!(
        children,
        &[XmlNode::Text("Represents a saved player.".to_owned())]
    );
}

#[test]
fn documentation_attaches_to_nominal_error_declarations() {
    let analysis = analyze(
        "namespace Io\n\
         --- <summary>\n\
         --- Describes failures while loading a file.\n\
         --- </summary>\n\
         public error LoadError\n\
             Missing(path: String)\n\
         end\n",
    );

    assert!(analysis.diagnostics().is_empty());
    assert_eq!(
        analysis.blocks()[0].target().expect("error target").kind(),
        NodeKind::ErrorDeclaration
    );
}

#[test]
fn blank_lines_and_ordinary_comments_break_attachment() {
    let blank = analyze(
        "namespace Saves\n\
         --- <summary>\n\
         --- Detached.\n\
         --- </summary>\n\
         \n\
         public record Save\n\
         end\n",
    );
    let ordinary = analyze(
        "namespace Saves\n\
         --- <summary>\n\
         --- Detached.\n\
         --- </summary>\n\
         -- ordinary comment\n\
         public record Save\n\
         end\n",
    );

    assert!(blank.blocks()[0].target().is_none());
    assert!(ordinary.blocks()[0].target().is_none());
}

#[test]
fn consecutive_lines_form_one_checked_xml_fragment() {
    let analysis = analyze(
        "namespace Saves\n\
         --- <summary>\n\
         --- Loads a save.\n\
         --- </summary>\n\
         ---\n\
         --- <param name=\"path\">\n\
         --- The path.\n\
         --- </param>\n\
         ---\n\
         --- <returns>\n\
         --- The save.\n\
         --- </returns>\n\
         public function load(path: String): String\n\
             return path\n\
         end\n",
    );

    assert_eq!(analysis.blocks().len(), 1);
    assert_eq!(analysis.blocks()[0].xml().expect("XML").children().len(), 3);
    assert!(analysis.diagnostics().is_empty());
}

#[test]
fn canonical_lines_preserve_inline_spacing_and_literal_code_whitespace() {
    let analysis = analyze(
        "namespace Saves\n\
         --- <summary>\n\
         --- Returns a <see cref=\"Player\"/> value.\n\
         --- </summary>\n\
         ---\n\
         --- <example>\n\
         --- <code>\n\
         ---     local value = 1\n\
         --- </code>\n\
         --- </example>\n\
         public record Save\n\
         end\n",
    );
    let xml = analysis.blocks()[0].xml().expect("checked XML");
    let XmlNode::Element {
        children: summary_children,
        ..
    } = &xml.children()[0]
    else {
        panic!("summary element");
    };
    assert!(matches!(&summary_children[0], XmlNode::Text(text) if text == "Returns a "));
    assert!(matches!(&summary_children[2], XmlNode::Text(text) if text == " value."));

    let XmlNode::Element {
        children: example_children,
        ..
    } = &xml.children()[1]
    else {
        panic!("example element");
    };
    let XmlNode::Element {
        name,
        children: code_children,
        ..
    } = &example_children[0]
    else {
        panic!("code element");
    };
    assert_eq!(name, "code");
    assert_eq!(
        code_children,
        &[XmlNode::Text("\n    local value = 1\n".to_owned())]
    );
}

#[test]
fn dtds_entities_and_processing_instructions_are_rejected() {
    for unsafe_xml in [
        "<!DOCTYPE summary><summary>Bad</summary>",
        "<!ENTITY x \"bad\"><summary>&x;</summary>",
        "<?xml version=\"1.0\"?><summary>Bad</summary>",
    ] {
        let text = format!("namespace Saves\n--- {unsafe_xml}\npublic record Save\nend\n");
        let analysis = analyze(&text);
        let codes: Vec<_> = analysis
            .diagnostics()
            .iter()
            .map(|diagnostic| diagnostic.code().as_str())
            .collect();
        assert_eq!(codes, ["POP6400"]);
        assert!(analysis.diagnostics().iter().all(|diagnostic| {
            diagnostic.severity() == DiagnosticSeverity::Warning
                && diagnostic.category() == DiagnosticCategory::Style
        }));
    }
}

#[test]
fn malformed_xml_has_a_deterministic_diagnostic() {
    let analysis = analyze(
        "namespace Saves\n\
         --- <summary>\n\
         --- Broken\n\
         --- </remarks>\n\
         public record Save\n\
         end\n",
    );

    assert_eq!(analysis.diagnostic_snapshot(), "POP6401@16..55\n");
    assert_eq!(
        analysis.diagnostics()[0].severity(),
        DiagnosticSeverity::Warning
    );
    assert_eq!(
        analysis.diagnostics()[0].category(),
        DiagnosticCategory::Style
    );
}

#[test]
fn typed_error_tags_require_exact_nominal_cases_and_complete_public_coverage() {
    let mut analysis = analyze(
        "namespace Io\n\
         --- <summary>\n\
         --- Loads a file.\n\
         --- </summary>\n\
         ---\n\
         --- <error type=\"LoadError.Missing\">\n\
         --- No file exists.\n\
         --- </error>\n\
         ---\n\
         --- <error type=\"OtherError\">\n\
         --- Wrong identity.\n\
         --- </error>\n\
         public function load(path: String): Int\n\
             return 0\n\
         end\n",
    );
    let target = analysis.blocks()[0]
        .target()
        .expect("function target")
        .range();
    analysis.validate_typed_errors(&[TypedErrorDocumentationContract::result(
        target,
        "LoadError",
        ["Missing", "Denied"],
        true,
    )]);

    let codes: Vec<_> = analysis
        .diagnostics()
        .iter()
        .map(|diagnostic| diagnostic.code().as_str())
        .collect();
    assert_eq!(codes, ["POP6402", "POP6403"]);
}

#[test]
fn error_tags_are_rejected_without_a_result_error_type() {
    let mut analysis = analyze(
        "namespace Io\n\
         --- <error type=\"LoadError\">\n\
         --- Not a result.\n\
         --- </error>\n\
         public function load(): Int\n\
             return 0\n\
         end\n",
    );
    let target = analysis.blocks()[0]
        .target()
        .expect("function target")
        .range();
    analysis.validate_typed_errors(&[TypedErrorDocumentationContract::without_result(target)]);

    assert_eq!(analysis.diagnostic_snapshot(), "POP6402@13..72\n");
}

#[test]
fn duplicate_error_tags_are_rejected_and_panic_does_not_cover_typed_errors() {
    let mut analysis = analyze(
        "namespace Io\n\
         --- <error type=\"LoadError.Missing\">\n\
         --- Missing.\n\
         --- </error>\n\
         ---\n\
         --- <error type=\"LoadError.Missing\">\n\
         --- Duplicate.\n\
         --- </error>\n\
         ---\n\
         --- <panic condition=\"denied\">\n\
         --- Invariant failure.\n\
         --- </panic>\n\
         public function load(): Int\n\
             return 0\n\
         end\n",
    );
    let target = analysis.blocks()[0]
        .target()
        .expect("function target")
        .range();
    analysis.validate_typed_errors(&[TypedErrorDocumentationContract::result(
        target,
        "LoadError",
        ["Missing", "Denied"],
        true,
    )]);

    assert_eq!(
        analysis
            .diagnostics()
            .iter()
            .map(|diagnostic| diagnostic.code().as_str())
            .collect::<Vec<_>>(),
        ["POP6402", "POP6403"]
    );
}

#[test]
fn public_error_declarations_and_cases_require_exactly_one_summary() {
    let mut analysis = analyze(
        "namespace Io\n\
         --- <summary>\n\
         --- Describes loading failures.\n\
         --- </summary>\n\
         public error LoadError\n\
             Missing(path: String)\n\
             --- <summary>\n\
             --- Access was denied.\n\
             --- </summary>\n\
             ---\n\
             --- <summary>\n\
             --- Duplicate summary.\n\
             --- </summary>\n\
             Denied\n\
         end\n",
    );
    let declaration = analysis.blocks()[0]
        .target()
        .expect("error declaration target")
        .range();
    let missing = analysis
        .blocks()
        .iter()
        .find(|block| block.xml_text().contains("Access was denied"))
        .expect("case documentation block")
        .range();
    let missing_case_start = missing.end();
    let source = "namespace Io\n\
                  --- <summary>\n\
                  --- Describes loading failures.\n\
                  --- </summary>\n\
                  public error LoadError\n\
                      Missing(path: String)\n\
                      --- <summary>\n\
                      --- Access was denied.\n\
                      --- </summary>\n\
                      ---\n\
                      --- <summary>\n\
                      --- Duplicate summary.\n\
                      --- </summary>\n\
                      Denied\n\
                  end\n";
    let missing_range = pop_foundation::TextRange::new(
        pop_foundation::TextSize::from_u32(
            u32::try_from(source.find("Missing(path").expect("missing case"))
                .expect("small source"),
        ),
        pop_foundation::TextSize::from_u32(
            u32::try_from(source.find("Missing(path").expect("missing case") + "Missing".len())
                .expect("small source"),
        ),
    )
    .expect("ordered range");
    let denied_range = pop_foundation::TextRange::new(
        pop_foundation::TextSize::from_u32(
            u32::try_from(source.find("Denied\n").expect("denied case")).expect("small source"),
        ),
        pop_foundation::TextSize::from_u32(
            u32::try_from(source.find("Denied\n").expect("denied case") + "Denied".len())
                .expect("small source"),
        ),
    )
    .expect("ordered range");
    assert!(missing_case_start < denied_range.start());

    analysis.validate_public_error_summaries(&[PublicErrorDocumentationContract::new(
        declaration,
        "LoadError",
        [("Missing", missing_range), ("Denied", denied_range)],
    )]);

    assert_eq!(
        analysis
            .diagnostics()
            .iter()
            .map(|diagnostic| diagnostic.code().as_str())
            .collect::<Vec<_>>(),
        ["POP6404", "POP6405"]
    );
}

#[test]
fn returns_tags_follow_the_typed_result_contract() {
    let mut no_result = analyze(
        "namespace Io\n\
         --- <returns>\n\
         --- Invalid.\n\
         --- </returns>\n\
         public function close()\n\
         end\n",
    );
    let no_result_target = no_result.blocks()[0]
        .target()
        .expect("function target")
        .range();
    no_result.validate_typed_returns(&[TypedReturnsDocumentationContract::without_result(
        no_result_target,
    )]);
    assert_eq!(no_result.diagnostic_snapshot(), "POP6408@13..54\n");

    let mut result = analyze(
        "namespace Io\n\
         --- <returns>\n\
         --- The successful value.\n\
         --- </returns>\n\
         public function load(): Result<Int, LoadError>\n\
             return Result.Error(LoadError.Failed())\n\
         end\n",
    );
    let result_target = result.blocks()[0]
        .target()
        .expect("function target")
        .range();
    result.validate_typed_returns(&[TypedReturnsDocumentationContract::result_ok(result_target)]);
    assert!(result.diagnostics().is_empty());

    let mut duplicate = analyze(
        "namespace Io\n\
         --- <returns>\n\
         --- First.\n\
         --- </returns>\n\
         ---\n\
         --- <returns>\n\
         --- Second.\n\
         --- </returns>\n\
         public function load(): Result<Int, LoadError>\n\
             return Result.Error(LoadError.Failed())\n\
         end\n",
    );
    let duplicate_target = duplicate.blocks()[0]
        .target()
        .expect("function target")
        .range();
    duplicate.validate_typed_returns(&[TypedReturnsDocumentationContract::result_ok(
        duplicate_target,
    )]);
    assert_eq!(
        duplicate
            .diagnostics()
            .iter()
            .map(|diagnostic| diagnostic.code().as_str())
            .collect::<Vec<_>>(),
        ["POP6408"]
    );
}
