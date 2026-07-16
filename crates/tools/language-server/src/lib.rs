//! Incremental language-server implementation.
//!
//! Public protocol types belong to the independently installed `Pop.Lsp`
//! Package. Compiler/query integration remains private to this tool crate.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use pop_documentation::{XmlFragment, XmlNode};
use pop_driver::{FrontEndBubbleInput, FrontEndModule, ToolingDeclarationKind, analyze_bubble};
use pop_foundation::{
    BubbleId, Diagnostic, DiagnosticSeverity, FileId, ModuleId, NamespaceId, TextRange, TextSize,
};
use pop_localization::{
    Argument, Language, LocalizationError, RenderContext, select_process_language,
};
use pop_query::CancellationToken;
use pop_source::SourceFile;

mod transport;

pub use transport::{ExitStatus, TransportError, TransportLimits, serve};

pub const PUBLIC_PROTOCOL_PACKAGE: &str = pop_extension_lsp::PACKAGE;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LanguageServerSession {
    rendering: RenderContext,
}

impl LanguageServerSession {
    /// Creates an immutable presentation session for one client locale.
    ///
    /// # Errors
    ///
    /// Returns an error when the requested locale is not supported.
    pub fn initialize(locale: Option<&str>) -> Result<Self, LocalizationError> {
        let language = match locale {
            Some(tag) => Language::from_tag(tag)
                .ok_or_else(|| LocalizationError::UnsupportedLanguage(tag.to_owned()))?,
            None => select_process_language(None)?,
        };
        Ok(Self {
            rendering: RenderContext::new(language),
        })
    }

    #[must_use]
    pub const fn language(self) -> Language {
        self.rendering.language()
    }

    /// Renders a structured compiler diagnostic in this session's language.
    ///
    /// # Errors
    ///
    /// Returns an error when a diagnostic key or argument schema does not match
    /// the embedded localization catalog.
    pub fn render_diagnostic(&self, diagnostic: &Diagnostic) -> Result<String, LocalizationError> {
        self.rendering.diagnostic(diagnostic)
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct DocumentUri(Arc<str>);

impl DocumentUri {
    /// Creates a bounded protocol URI identity.
    ///
    /// # Errors
    ///
    /// Returns [`DocumentUriError`] for an empty, relative, or control-bearing
    /// value.
    pub fn new(value: impl Into<Arc<str>>) -> Result<Self, DocumentUriError> {
        let value = value.into();
        if value.is_empty() || !value.contains(':') || value.chars().any(char::is_control) {
            return Err(DocumentUriError);
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DocumentUriError;

impl fmt::Display for DocumentUriError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .write_str("document URI must be nonempty, absolute, and free of control characters")
    }
}

impl std::error::Error for DocumentUriError {}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct DocumentVersion(u64);

impl DocumentVersion {
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn value(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProtocolPosition {
    line: u32,
    character: u32,
}

impl ProtocolPosition {
    #[must_use]
    pub const fn new(line: u32, character: u32) -> Self {
        Self { line, character }
    }

    #[must_use]
    pub const fn line(self) -> u32 {
        self.line
    }

    #[must_use]
    pub const fn character(self) -> u32 {
        self.character
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProtocolRange {
    start: ProtocolPosition,
    end: ProtocolPosition,
}

impl ProtocolRange {
    #[must_use]
    pub const fn start(self) -> ProtocolPosition {
        self.start
    }

    #[must_use]
    pub const fn end(self) -> ProtocolPosition {
        self.end
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProtocolDiagnostic {
    code: String,
    severity: DiagnosticSeverity,
    range: ProtocolRange,
    message: String,
}

impl ProtocolDiagnostic {
    #[must_use]
    pub fn code(&self) -> &str {
        &self.code
    }

    #[must_use]
    pub const fn severity(&self) -> DiagnosticSeverity {
        self.severity
    }

    #[must_use]
    pub const fn range(&self) -> ProtocolRange {
        self.range
    }

    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DocumentAnalysis {
    file: FileId,
    version: DocumentVersion,
    diagnostics: Vec<ProtocolDiagnostic>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Hover {
    signature: String,
    summary: Option<String>,
    range: ProtocolRange,
}

impl Hover {
    #[must_use]
    pub fn signature(&self) -> &str {
        &self.signature
    }

    #[must_use]
    pub fn summary(&self) -> Option<&str> {
        self.summary.as_deref()
    }

    #[must_use]
    pub const fn range(&self) -> ProtocolRange {
        self.range
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DocumentSymbol {
    name: String,
    kind: &'static str,
    range: ProtocolRange,
    selection_range: ProtocolRange,
}

impl DocumentSymbol {
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub const fn kind(&self) -> &'static str {
        self.kind
    }

    #[must_use]
    pub const fn range(&self) -> ProtocolRange {
        self.range
    }

    #[must_use]
    pub const fn selection_range(&self) -> ProtocolRange {
        self.selection_range
    }
}

impl DocumentAnalysis {
    #[must_use]
    pub const fn file(&self) -> FileId {
        self.file
    }

    #[must_use]
    pub const fn version(&self) -> DocumentVersion {
        self.version
    }

    #[must_use]
    pub fn diagnostics(&self) -> &[ProtocolDiagnostic] {
        &self.diagnostics
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LanguageServerError {
    DocumentAlreadyOpen {
        uri: DocumentUri,
    },
    DocumentNotOpen {
        uri: DocumentUri,
    },
    StaleVersion {
        uri: DocumentUri,
        current: DocumentVersion,
        received: DocumentVersion,
    },
    SourceRejected {
        uri: DocumentUri,
        detail: String,
    },
    Cancelled,
    Localization(String),
    TooManyDocuments,
    DocumentTooLarge {
        uri: DocumentUri,
        length: u64,
        limit: u64,
    },
}

struct OpenDocument {
    source: SourceFile,
    version: DocumentVersion,
    analysis: DocumentAnalysis,
    declarations: Vec<AnalyzedDeclaration>,
}

struct AnalyzedDeclaration {
    name: String,
    kind: &'static str,
    declaration: TextRange,
    selection: TextRange,
    signature: String,
    summary: Option<String>,
}

pub struct LanguageServer {
    session: LanguageServerSession,
    documents: BTreeMap<DocumentUri, OpenDocument>,
    next_file: u32,
}

impl LanguageServer {
    /// Creates an empty language-server session for one client locale.
    ///
    /// # Errors
    ///
    /// Returns an error when the requested locale is not supported.
    pub fn initialize(locale: Option<&str>) -> Result<Self, LocalizationError> {
        Ok(Self {
            session: LanguageServerSession::initialize(locale)?,
            documents: BTreeMap::new(),
            next_file: 0,
        })
    }

    #[must_use]
    pub fn document_count(&self) -> usize {
        self.documents.len()
    }

    /// Opens and analyzes a new versioned document snapshot atomically.
    ///
    /// # Errors
    ///
    /// Rejects cancellation, duplicate URIs, invalid source snapshots,
    /// localization failures, and exhausted session file identities.
    pub fn open(
        &mut self,
        uri: DocumentUri,
        version: DocumentVersion,
        text: impl Into<Arc<str>>,
        cancellation: &CancellationToken,
    ) -> Result<DocumentAnalysis, LanguageServerError> {
        cancellation
            .check()
            .map_err(|_| LanguageServerError::Cancelled)?;
        if self.documents.contains_key(&uri) {
            return Err(LanguageServerError::DocumentAlreadyOpen { uri });
        }
        let file = FileId::from_raw(self.next_file);
        let source =
            SourceFile::new(file, Arc::<str>::from(uri.as_str()), text).map_err(|error| {
                LanguageServerError::SourceRejected {
                    uri: uri.clone(),
                    detail: error.to_string(),
                }
            })?;
        let (analysis, declarations) =
            analyze_document(self.session, &source, version, cancellation)?;
        self.next_file = self
            .next_file
            .checked_add(1)
            .ok_or(LanguageServerError::TooManyDocuments)?;
        self.documents.insert(
            uri,
            OpenDocument {
                source,
                version,
                analysis: analysis.clone(),
                declarations,
            },
        );
        Ok(analysis)
    }

    /// Replaces an open document with a strictly newer full-text snapshot.
    ///
    /// # Errors
    ///
    /// Rejects cancellation, unknown documents, stale versions, invalid source
    /// snapshots, and localization failures without publishing partial state.
    pub fn change(
        &mut self,
        uri: &DocumentUri,
        version: DocumentVersion,
        text: impl Into<Arc<str>>,
        cancellation: &CancellationToken,
    ) -> Result<DocumentAnalysis, LanguageServerError> {
        cancellation
            .check()
            .map_err(|_| LanguageServerError::Cancelled)?;
        let document = self
            .documents
            .get(uri)
            .ok_or_else(|| LanguageServerError::DocumentNotOpen { uri: uri.clone() })?;
        if version <= document.version {
            return Err(LanguageServerError::StaleVersion {
                uri: uri.clone(),
                current: document.version,
                received: version,
            });
        }
        let source = SourceFile::new(document.source.id(), Arc::<str>::from(uri.as_str()), text)
            .map_err(|error| LanguageServerError::SourceRejected {
                uri: uri.clone(),
                detail: error.to_string(),
            })?;
        let (analysis, declarations) =
            analyze_document(self.session, &source, version, cancellation)?;
        self.documents.insert(
            uri.clone(),
            OpenDocument {
                source,
                version,
                analysis: analysis.clone(),
                declarations,
            },
        );
        Ok(analysis)
    }

    /// Reanalyzes the currently published snapshot for a document.
    ///
    /// # Errors
    ///
    /// Rejects unknown documents, cancellation, and localization failures.
    pub fn analyze(
        &self,
        uri: &DocumentUri,
        cancellation: &CancellationToken,
    ) -> Result<DocumentAnalysis, LanguageServerError> {
        let document = self
            .documents
            .get(uri)
            .ok_or_else(|| LanguageServerError::DocumentNotOpen { uri: uri.clone() })?;
        cancellation
            .check()
            .map_err(|_| LanguageServerError::Cancelled)?;
        Ok(document.analysis.clone())
    }

    /// Returns compiler-checked hover information for a namespace declaration.
    ///
    /// # Errors
    ///
    /// Rejects unknown documents, cancellation, or invalid UTF-16 positions.
    pub fn hover(
        &self,
        uri: &DocumentUri,
        position: ProtocolPosition,
        cancellation: &CancellationToken,
    ) -> Result<Option<Hover>, LanguageServerError> {
        cancellation
            .check()
            .map_err(|_| LanguageServerError::Cancelled)?;
        let document = self
            .documents
            .get(uri)
            .ok_or_else(|| LanguageServerError::DocumentNotOpen { uri: uri.clone() })?;
        let Some(offset) = source_offset(document.source.text(), position) else {
            return Ok(None);
        };
        let Some(declaration) = document.declarations.iter().find(|declaration| {
            declaration.selection.start() <= offset && offset < declaration.selection.end()
        }) else {
            return Ok(None);
        };
        Ok(Some(Hover {
            signature: declaration.signature.clone(),
            summary: declaration.summary.clone(),
            range: protocol_range(document.source.text(), declaration.selection)?,
        }))
    }

    /// Returns compiler-indexed namespace declarations for the open Module.
    ///
    /// # Errors
    ///
    /// Rejects unknown documents, cancellation, or invalid compiler spans.
    pub fn document_symbols(
        &self,
        uri: &DocumentUri,
        cancellation: &CancellationToken,
    ) -> Result<Vec<DocumentSymbol>, LanguageServerError> {
        cancellation
            .check()
            .map_err(|_| LanguageServerError::Cancelled)?;
        let document = self
            .documents
            .get(uri)
            .ok_or_else(|| LanguageServerError::DocumentNotOpen { uri: uri.clone() })?;
        document
            .declarations
            .iter()
            .map(|declaration| {
                Ok(DocumentSymbol {
                    name: declaration.name.clone(),
                    kind: declaration.kind,
                    range: protocol_range(document.source.text(), declaration.declaration)?,
                    selection_range: protocol_range(document.source.text(), declaration.selection)?,
                })
            })
            .collect()
    }

    pub fn close(&mut self, uri: &DocumentUri) -> bool {
        self.documents.remove(uri).is_some()
    }

    /// Renders a language-server error with this session's catalog.
    ///
    /// # Errors
    ///
    /// Returns an error when the corresponding localization key or typed
    /// arguments do not match the embedded catalog.
    pub fn render_error(&self, error: &LanguageServerError) -> Result<String, LocalizationError> {
        let context = self.session.rendering;
        match error {
            LanguageServerError::DocumentAlreadyOpen { uri } => context.message(
                "lsp.documentAlreadyOpen",
                &[Argument::text("uri", uri.as_str())],
            ),
            LanguageServerError::DocumentNotOpen { uri } => context.message(
                "lsp.documentNotOpen",
                &[Argument::text("uri", uri.as_str())],
            ),
            LanguageServerError::StaleVersion {
                uri,
                current,
                received,
            } => context.message(
                "lsp.staleDocument",
                &[
                    Argument::text("uri", uri.as_str()),
                    Argument::unsigned("found", received.value()),
                    Argument::unsigned("expected", current.value()),
                ],
            ),
            LanguageServerError::SourceRejected { uri, detail } => context.message(
                "lsp.sourceRejected",
                &[
                    Argument::text("uri", uri.as_str()),
                    Argument::external("detail", detail),
                ],
            ),
            LanguageServerError::Cancelled => context.message("lsp.cancelled", &[]),
            LanguageServerError::Localization(detail) => {
                Err(LocalizationError::InvalidArguments(detail.clone()))
            }
            LanguageServerError::TooManyDocuments => context.message("lsp.tooManyDocuments", &[]),
            LanguageServerError::DocumentTooLarge { uri, length, limit } => context.message(
                "lsp.documentTooLarge",
                &[
                    Argument::text("uri", uri.as_str()),
                    Argument::unsigned("length", *length),
                    Argument::unsigned("limit", *limit),
                ],
            ),
        }
    }
}

fn analyze_document(
    session: LanguageServerSession,
    source: &SourceFile,
    version: DocumentVersion,
    cancellation: &CancellationToken,
) -> Result<(DocumentAnalysis, Vec<AnalyzedDeclaration>), LanguageServerError> {
    cancellation
        .check()
        .map_err(|_| LanguageServerError::Cancelled)?;
    let result = analyze_bubble(FrontEndBubbleInput::new(
        BubbleId::from_raw(0),
        NamespaceId::from_raw(0),
        Vec::new(),
        vec![FrontEndModule::new(
            ModuleId::from_raw(source.id().raw()),
            source.clone(),
        )],
    ));
    cancellation
        .check()
        .map_err(|_| LanguageServerError::Cancelled)?;
    let diagnostics = result
        .diagnostics()
        .iter()
        .map(|diagnostic| protocol_diagnostic(session, source, diagnostic))
        .collect::<Result<Vec<_>, _>>()?;
    cancellation
        .check()
        .map_err(|_| LanguageServerError::Cancelled)?;
    let documentation = result
        .checked_documentation()
        .iter()
        .map(|documentation| (documentation.identity(), documentation.fragment()))
        .collect::<BTreeMap<_, _>>();
    let declarations = result
        .tooling_declarations()
        .iter()
        .map(|declaration| {
            let signature_range = declaration.signature_span().range();
            let signature = source
                .text()
                .get(signature_range.start().to_usize()..signature_range.end().to_usize())
                .unwrap_or_default()
                .trim()
                .to_owned();
            AnalyzedDeclaration {
                name: declaration.name().to_owned(),
                kind: declaration_kind(declaration.kind()),
                declaration: declaration.declaration_span().range(),
                selection: declaration.selection_span().range(),
                signature,
                summary: documentation
                    .get(&declaration.identity())
                    .and_then(|fragment| documentation_summary(fragment)),
            }
        })
        .collect();
    Ok((
        DocumentAnalysis {
            file: source.id(),
            version,
            diagnostics,
        },
        declarations,
    ))
}

const fn declaration_kind(kind: ToolingDeclarationKind) -> &'static str {
    match kind {
        ToolingDeclarationKind::Function => "function",
        ToolingDeclarationKind::Constant => "constant",
        ToolingDeclarationKind::TypeAlias => "type alias",
        ToolingDeclarationKind::Attribute => "attribute",
        ToolingDeclarationKind::Record => "record",
        ToolingDeclarationKind::Union => "union",
        ToolingDeclarationKind::Error => "error",
        ToolingDeclarationKind::Class => "class",
        ToolingDeclarationKind::Interface => "interface",
        ToolingDeclarationKind::Enum => "enum",
    }
}

fn documentation_summary(fragment: &XmlFragment) -> Option<String> {
    fragment.children().iter().find_map(|node| match node {
        XmlNode::Element { name, children, .. } if name == "summary" => {
            let mut text = String::new();
            collect_documentation_text(children, &mut text);
            let normalized = text.split_whitespace().collect::<Vec<_>>().join(" ");
            (!normalized.is_empty()).then_some(normalized)
        }
        _ => None,
    })
}

fn collect_documentation_text(nodes: &[XmlNode], output: &mut String) {
    for node in nodes {
        match node {
            XmlNode::Text(text) => output.push_str(text),
            XmlNode::Element { children, .. } => collect_documentation_text(children, output),
        }
    }
}

fn protocol_range(text: &str, range: TextRange) -> Result<ProtocolRange, LanguageServerError> {
    let start = protocol_position(text, range.start())
        .ok_or_else(|| LanguageServerError::Localization("invalid tooling start".to_owned()))?;
    let end = protocol_position(text, range.end())
        .ok_or_else(|| LanguageServerError::Localization("invalid tooling end".to_owned()))?;
    Ok(ProtocolRange { start, end })
}

fn source_offset(text: &str, position: ProtocolPosition) -> Option<TextSize> {
    let mut line_start = 0_usize;
    for _ in 0..position.line {
        let newline = text.get(line_start..)?.find('\n')?;
        line_start = line_start.checked_add(newline)?.checked_add(1)?;
    }
    let line = text
        .get(line_start..)?
        .split_once('\n')
        .map_or(text.get(line_start..)?, |(line, _)| line);
    let content = line;
    let mut utf16 = 0_u32;
    for (byte, character) in content.char_indices() {
        if utf16 == position.character {
            return TextSize::try_from_usize(line_start + byte);
        }
        utf16 = utf16.checked_add(u32::try_from(character.len_utf16()).ok()?)?;
        if utf16 > position.character {
            return None;
        }
    }
    (utf16 == position.character)
        .then(|| TextSize::try_from_usize(line_start + content.len()))
        .flatten()
}

fn protocol_diagnostic(
    session: LanguageServerSession,
    source: &SourceFile,
    diagnostic: &Diagnostic,
) -> Result<ProtocolDiagnostic, LanguageServerError> {
    let range = diagnostic.primary_span().range();
    let start = protocol_position(source.text(), range.start())
        .ok_or_else(|| LanguageServerError::Localization("invalid diagnostic start".to_owned()))?;
    let end = protocol_position(source.text(), range.end())
        .ok_or_else(|| LanguageServerError::Localization("invalid diagnostic end".to_owned()))?;
    let message = session
        .rendering
        .diagnostic_message_only(diagnostic)
        .map_err(|error| LanguageServerError::Localization(error.to_string()))?;
    Ok(ProtocolDiagnostic {
        code: diagnostic.code().as_str().to_owned(),
        severity: diagnostic.severity(),
        range: ProtocolRange { start, end },
        message,
    })
}

fn protocol_position(text: &str, offset: TextSize) -> Option<ProtocolPosition> {
    let offset = offset.to_usize();
    if offset > text.len() || !text.is_char_boundary(offset) {
        return None;
    }
    let prefix = &text[..offset];
    let line = u32::try_from(prefix.bytes().filter(|byte| *byte == b'\n').count()).ok()?;
    let line_text = prefix.rsplit_once('\n').map_or(prefix, |(_, line)| line);
    let character = u32::try_from(line_text.encode_utf16().count()).ok()?;
    Some(ProtocolPosition::new(line, character))
}
