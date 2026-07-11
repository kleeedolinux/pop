use pop_documentation::{DocumentationAnalysis, XmlNode};
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
         --- <summary>Represents a saved player.</summary>\n\
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
        "<summary>Represents a saved player.</summary>"
    );
    assert!(matches!(
        block.xml().expect("safe XML").children()[0],
        XmlNode::Element { ref name, .. } if name == "summary"
    ));
}

#[test]
fn blank_lines_and_ordinary_comments_break_attachment() {
    let blank = analyze(
        "namespace Saves\n\
         --- <summary>Detached.</summary>\n\
         \n\
         public record Save\n\
         end\n",
    );
    let ordinary = analyze(
        "namespace Saves\n\
         --- <summary>Detached.</summary>\n\
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
         --- <summary>Loads a save.</summary>\n\
         --- <param name=\"path\">The path.</param>\n\
         --- <returns>The save.</returns>\n\
         public function load(path: String): String\n\
             return path\n\
         end\n",
    );

    assert_eq!(analysis.blocks().len(), 1);
    assert_eq!(analysis.blocks()[0].xml().expect("XML").children().len(), 3);
    assert!(analysis.diagnostics().is_empty());
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
         --- <summary>Broken</remarks>\n\
         public record Save\n\
         end\n",
    );

    assert_eq!(analysis.diagnostic_snapshot(), "POP6401@16..45\n");
    assert_eq!(
        analysis.diagnostics()[0].severity(),
        DiagnosticSeverity::Warning
    );
    assert_eq!(
        analysis.diagnostics()[0].category(),
        DiagnosticCategory::Style
    );
}
