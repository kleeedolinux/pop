use std::num::NonZeroUsize;

use pop_diagnostics::{
    DiagnosticPolicy, DiagnosticReport, DiagnosticSelector, DocumentSnapshot, FixAllError,
    WarningGroup, WorkspaceSnapshot, apply_safe_fix_all, documentation,
};
use pop_foundation::{
    Diagnostic, DiagnosticCategory, DiagnosticCode, DiagnosticSeverity, FileId, FixApplicability,
    MessageKey, QuickFix, SourceSpan, TextEdit, TextRange, TextSize, WorkspaceEdit,
};

fn range(start: u32, end: u32) -> TextRange {
    TextRange::new(TextSize::from_u32(start), TextSize::from_u32(end)).expect("ordered range")
}

fn span(file: u32, start: u32, end: u32) -> SourceSpan {
    SourceSpan::new(FileId::from_raw(file), range(start, end))
}

fn error(code: &'static str, file: u32, start: u32) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCode::new(code),
        DiagnosticSeverity::Error,
        DiagnosticCategory::Type,
        MessageKey::new("types.notAllPathsReturn"),
        Vec::new(),
        span(file, start, start + 1),
    )
}

#[test]
fn warning_policy_gates_waves_and_promotes_without_mutating_intrinsic_severity() {
    let warning = documentation::missing_summary(span(0, 4, 8), "Player");
    assert_eq!(
        warning
            .warning_wave()
            .map(pop_foundation::WarningWave::value),
        Some(1)
    );
    assert_eq!(
        warning
            .suppression_key()
            .map(pop_foundation::SuppressionKey::as_str),
        Some("POP6404")
    );

    let disabled_by_wave = DiagnosticPolicy::new(0).evaluate(&warning);
    assert!(!disabled_by_wave.is_enabled());
    assert!(!disabled_by_wave.blocks_artifact());

    let ordinary = DiagnosticPolicy::new(1).evaluate(&warning);
    assert!(ordinary.is_enabled());
    assert!(!ordinary.blocks_artifact());

    let promoted = DiagnosticPolicy::new(1)
        .with_warnings_as_errors([DiagnosticSelector::Group(WarningGroup::Documentation)])
        .evaluate(&warning);
    assert!(promoted.is_enabled());
    assert!(promoted.blocks_artifact());
    assert!(promoted.is_promoted());
    assert_eq!(warning.severity(), DiagnosticSeverity::Warning);

    let disabled = DiagnosticPolicy::new(1)
        .with_warnings_as_errors([DiagnosticSelector::All])
        .with_disabled_warnings([DiagnosticSelector::code("POP6404").expect("known code")])
        .evaluate(&warning);
    assert!(!disabled.is_enabled());
    assert!(!disabled.blocks_artifact());

    let source_error = error("POP2006", 0, 10);
    let attempted_suppression = DiagnosticPolicy::new(1)
        .with_disabled_warnings([DiagnosticSelector::All])
        .evaluate(&source_error);
    assert!(attempted_suppression.is_enabled());
    assert!(attempted_suppression.blocks_artifact());
}

#[test]
fn report_bounds_effective_errors_but_preserves_warning_facts_and_counts() {
    let diagnostics = vec![
        error("POP2006", 0, 1),
        documentation::missing_summary(span(0, 2, 3), "Player"),
        error("POP2006", 0, 4),
        error("POP2006", 0, 6),
    ];
    let report = DiagnosticReport::new(
        &diagnostics,
        &DiagnosticPolicy::new(1),
        NonZeroUsize::new(2).expect("non-zero limit"),
    );

    assert_eq!(report.diagnostics().len(), 3);
    assert_eq!(report.error_count(), 3);
    assert_eq!(report.warning_count(), 1);
    assert_eq!(report.omitted_error_count(), 1);
    assert!(report.reached_error_limit());
    assert!(report.blocks_artifact());
    assert_eq!(
        report
            .diagnostics()
            .iter()
            .map(|diagnostic| diagnostic.code().as_str())
            .collect::<Vec<_>>(),
        ["POP2006", "POP6404", "POP2006"]
    );
}

fn safe_fix(
    id: &'static str,
    file: u32,
    revision: u64,
    edit_range: TextRange,
    replacement: &str,
) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCode::new("POP0004"),
        DiagnosticSeverity::Error,
        DiagnosticCategory::Syntax,
        MessageKey::new("syntax.unsupportedExport"),
        Vec::new(),
        SourceSpan::new(FileId::from_raw(file), edit_range),
    )
    .with_fix(
        QuickFix::new(
            id,
            MessageKey::new("fix.replaceExportWithPublic"),
            FixApplicability::Safe,
            WorkspaceEdit::new(
                revision,
                vec![TextEdit::new(
                    FileId::from_raw(file),
                    edit_range,
                    replacement,
                )],
            ),
        )
        .with_fix_all_equivalence("completeName"),
    )
}

#[test]
fn safe_fix_all_composes_deterministically_and_commits_only_after_postcondition() {
    let file = FileId::from_raw(3);
    let mut workspace =
        WorkspaceSnapshot::new([DocumentSnapshot::new(file, 7, "Iter Iter".to_owned())])
            .expect("unique documents");
    let diagnostics = vec![
        safe_fix("expandIterator", 3, 7, range(5, 9), "Iterator"),
        safe_fix("expandIterable", 3, 7, range(0, 4), "Iterable"),
    ];

    let summary = apply_safe_fix_all(&mut workspace, &diagnostics, |candidate| {
        candidate.document(file).is_some_and(|document| {
            document.text() == "Iterable Iterator" && document.revision() == 8
        })
    })
    .expect("safe edits compose");

    assert_eq!(summary.applied_fix_count(), 2);
    assert_eq!(summary.changed_document_count(), 1);
    assert_eq!(workspace.document(file).expect("document").revision(), 8);
    assert_eq!(
        workspace.document(file).expect("document").text(),
        "Iterable Iterator"
    );
}

#[test]
fn safe_fix_all_rejects_conflicts_stale_versions_and_failed_postconditions_atomically() {
    let file = FileId::from_raw(3);
    let original = WorkspaceSnapshot::new([DocumentSnapshot::new(file, 7, "export".to_owned())])
        .expect("workspace");

    let conflicting = vec![
        safe_fix("first", 3, 7, range(0, 6), "public"),
        safe_fix("second", 3, 7, range(0, 6), "internal"),
    ];
    let mut workspace = original.clone();
    assert!(matches!(
        apply_safe_fix_all(&mut workspace, &conflicting, |_| true),
        Err(FixAllError::ConflictingEdits { .. })
    ));
    assert_eq!(workspace, original);

    let stale = vec![safe_fix("stale", 3, 6, range(0, 6), "public")];
    assert!(matches!(
        apply_safe_fix_all(&mut workspace, &stale, |_| true),
        Err(FixAllError::StaleDocument { .. })
    ));
    assert_eq!(workspace, original);

    let valid = vec![safe_fix("valid", 3, 7, range(0, 6), "public")];
    assert_eq!(
        apply_safe_fix_all(&mut workspace, &valid, |_| false),
        Err(FixAllError::PostconditionFailed)
    );
    assert_eq!(workspace, original);
}

#[test]
fn unattended_fix_all_skips_review_unsafe_and_unproven_fixes() {
    let file = FileId::from_raw(0);
    let mut workspace =
        WorkspaceSnapshot::new([DocumentSnapshot::new(file, 0, "export".to_owned())])
            .expect("workspace");
    let diagnostic = [
        FixApplicability::RequiresReview,
        FixApplicability::Unsafe,
        FixApplicability::Safe,
    ]
    .into_iter()
    .fold(
        Diagnostic::new(
            DiagnosticCode::new("POP0004"),
            DiagnosticSeverity::Error,
            DiagnosticCategory::Syntax,
            MessageKey::new("syntax.unsupportedExport"),
            Vec::new(),
            span(0, 0, 6),
        ),
        |diagnostic, applicability| {
            diagnostic.with_fix(QuickFix::new(
                "notUnattended",
                MessageKey::new("fix.replaceExportWithPublic"),
                applicability,
                WorkspaceEdit::new(0, vec![TextEdit::new(file, range(0, 6), "public")]),
            ))
        },
    );

    let summary =
        apply_safe_fix_all(&mut workspace, &[diagnostic], |_| true).expect("nothing to apply");
    assert_eq!(summary.applied_fix_count(), 0);
    assert_eq!(summary.skipped_review_count(), 1);
    assert_eq!(summary.skipped_unsafe_count(), 1);
    assert_eq!(summary.skipped_unproven_count(), 1);
    assert_eq!(workspace.document(file).expect("document").text(), "export");
}
