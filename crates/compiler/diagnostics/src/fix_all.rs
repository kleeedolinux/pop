use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;

use pop_foundation::{Diagnostic, FileId, FixApplicability, TextEdit};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DocumentSnapshot {
    file: FileId,
    revision: u64,
    text: String,
    writable: bool,
}

impl DocumentSnapshot {
    #[must_use]
    pub const fn new(file: FileId, revision: u64, text: String) -> Self {
        Self {
            file,
            revision,
            text,
            writable: true,
        }
    }

    #[must_use]
    pub const fn read_only(mut self) -> Self {
        self.writable = false;
        self
    }

    #[must_use]
    pub const fn file(&self) -> FileId {
        self.file
    }

    #[must_use]
    pub const fn revision(&self) -> u64 {
        self.revision
    }

    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    #[must_use]
    pub const fn is_writable(&self) -> bool {
        self.writable
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkspaceSnapshot {
    documents: BTreeMap<FileId, DocumentSnapshot>,
}

impl WorkspaceSnapshot {
    /// Creates a versioned immutable-input workspace snapshot.
    ///
    /// # Errors
    ///
    /// Returns the duplicate file identity when two documents claim it.
    pub fn new(documents: impl IntoIterator<Item = DocumentSnapshot>) -> Result<Self, FileId> {
        let mut indexed = BTreeMap::new();
        for document in documents {
            let file = document.file();
            if indexed.insert(file, document).is_some() {
                return Err(file);
            }
        }
        Ok(Self { documents: indexed })
    }

    #[must_use]
    pub fn document(&self, file: FileId) -> Option<&DocumentSnapshot> {
        self.documents.get(&file)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FixAllError {
    UnknownDocument {
        file: FileId,
    },
    StaleDocument {
        file: FileId,
        expected: u64,
        found: u64,
    },
    ReadOnlyDocument {
        file: FileId,
    },
    InvalidRange {
        file: FileId,
    },
    ConflictingEdits {
        file: FileId,
    },
    RevisionOverflow {
        file: FileId,
    },
    PostconditionFailed,
}

impl fmt::Display for FixAllError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownDocument { file } => {
                write!(formatter, "fix names unknown file#{}", file.raw())
            }
            Self::StaleDocument {
                file,
                expected,
                found,
            } => write!(
                formatter,
                "fix for file#{} expects revision {expected}, found {found}",
                file.raw()
            ),
            Self::ReadOnlyDocument { file } => {
                write!(formatter, "fix cannot edit read-only file#{}", file.raw())
            }
            Self::InvalidRange { file } => {
                write!(formatter, "fix has an invalid range in file#{}", file.raw())
            }
            Self::ConflictingEdits { file } => {
                write!(formatter, "fixes conflict in file#{}", file.raw())
            }
            Self::RevisionOverflow { file } => {
                write!(formatter, "file#{} revision cannot advance", file.raw())
            }
            Self::PostconditionFailed => formatter.write_str("safe fix postcondition failed"),
        }
    }
}

impl Error for FixAllError {}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct FixAllSummary {
    applied_fixes: usize,
    changed_documents: usize,
    skipped_review: usize,
    skipped_unsafe: usize,
    skipped_unproven: usize,
}

impl FixAllSummary {
    #[must_use]
    pub const fn applied_fix_count(self) -> usize {
        self.applied_fixes
    }

    #[must_use]
    pub const fn changed_document_count(self) -> usize {
        self.changed_documents
    }

    #[must_use]
    pub const fn skipped_review_count(self) -> usize {
        self.skipped_review
    }

    #[must_use]
    pub const fn skipped_unsafe_count(self) -> usize {
        self.skipped_unsafe
    }

    #[must_use]
    pub const fn skipped_unproven_count(self) -> usize {
        self.skipped_unproven
    }
}

/// Plans, verifies, and commits all proven composing safe fixes as one
/// in-memory transaction.
///
/// # Errors
///
/// Returns an error without mutating `workspace` when any edit is stale,
/// invalid, read-only, conflicting, or fails the supplied semantic
/// postcondition.
pub fn apply_safe_fix_all(
    workspace: &mut WorkspaceSnapshot,
    diagnostics: &[Diagnostic],
    verify: impl FnOnce(&WorkspaceSnapshot) -> bool,
) -> Result<FixAllSummary, FixAllError> {
    let mut summary = FixAllSummary::default();
    let mut seen = BTreeSet::new();
    let mut edits_by_file: BTreeMap<FileId, Vec<&TextEdit>> = BTreeMap::new();
    for fix in diagnostics.iter().flat_map(Diagnostic::fixes) {
        match fix.applicability() {
            FixApplicability::RequiresReview => {
                summary.skipped_review += 1;
                continue;
            }
            FixApplicability::Unsafe => {
                summary.skipped_unsafe += 1;
                continue;
            }
            FixApplicability::Safe if fix.fix_all_equivalence().is_none() => {
                summary.skipped_unproven += 1;
                continue;
            }
            FixApplicability::Safe => {}
        }
        for edit in fix.edit().edits() {
            let document = workspace
                .document(edit.file())
                .ok_or(FixAllError::UnknownDocument { file: edit.file() })?;
            if document.revision() != fix.edit().revision() {
                return Err(FixAllError::StaleDocument {
                    file: edit.file(),
                    expected: fix.edit().revision(),
                    found: document.revision(),
                });
            }
            if !document.is_writable() {
                return Err(FixAllError::ReadOnlyDocument { file: edit.file() });
            }
            edits_by_file.entry(edit.file()).or_default().push(edit);
        }
        if seen.insert((fix.id(), fix.fix_all_equivalence())) {
            summary.applied_fixes += 1;
        }
    }

    if edits_by_file.is_empty() {
        return Ok(summary);
    }

    let mut candidate = workspace.clone();
    for (file, edits) in &mut edits_by_file {
        edits.sort_by_key(|edit| (edit.range().start(), edit.range().end(), edit.replacement()));
        edits.dedup_by(|left, right| *left == *right);
        let document = candidate
            .documents
            .get_mut(file)
            .ok_or(FixAllError::UnknownDocument { file: *file })?;
        validate_edits(*file, &document.text, edits)?;
        for edit in edits.iter().rev() {
            document.text.replace_range(
                edit.range().start().to_usize()..edit.range().end().to_usize(),
                edit.replacement(),
            );
        }
        document.revision = document
            .revision
            .checked_add(1)
            .ok_or(FixAllError::RevisionOverflow { file: *file })?;
        summary.changed_documents += 1;
    }
    if !verify(&candidate) {
        return Err(FixAllError::PostconditionFailed);
    }
    *workspace = candidate;
    Ok(summary)
}

fn validate_edits(file: FileId, text: &str, edits: &[&TextEdit]) -> Result<(), FixAllError> {
    let mut previous = None;
    for edit in edits {
        let start = edit.range().start().to_usize();
        let end = edit.range().end().to_usize();
        if end > text.len() || !text.is_char_boundary(start) || !text.is_char_boundary(end) {
            return Err(FixAllError::InvalidRange { file });
        }
        if let Some((previous_start, previous_end)) = previous
            && (start < previous_end
                || (start == previous_start && start == end && previous_start == previous_end))
        {
            return Err(FixAllError::ConflictingEdits { file });
        }
        previous = Some((start, end));
    }
    Ok(())
}
