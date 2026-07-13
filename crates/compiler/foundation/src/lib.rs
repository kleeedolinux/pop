//! Foundation IDs, spans, diagnostics, and deterministic utilities.
//!
//! This crate deliberately contains no language-semantic or backend policy.

use std::fmt;

macro_rules! typed_id {
    ($($name:ident),+ $(,)?) => {
        $(
            #[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
            pub struct $name(u32);

            impl $name {
                #[must_use]
                pub const fn from_raw(raw: u32) -> Self {
                    Self(raw)
                }

                #[must_use]
                pub const fn raw(self) -> u32 {
                    self.0
                }
            }
        )+
    };
}

typed_id!(
    WorkspaceId,
    PackageId,
    BubbleId,
    ModuleId,
    FileId,
    SpanId,
    SymbolId,
    StandardFunctionId,
    TypeId,
    BuiltinTypeId,
    ClassId,
    InterfaceId,
    InterfaceMethodId,
    FieldId,
    EnumCaseId,
    UnionCaseId,
    MethodId,
    ParameterId,
    ValueParameterId,
    LocalId,
    BindingId,
    CaptureId,
    NestedFunctionId,
    OpaqueId,
    NamespaceId,
    AttributeId,
    FunctionId,
    BlockId,
    ValueId,
);

/// Stable identity of one declaration inside its owning Bubble.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SymbolIdentity {
    bubble: BubbleId,
    symbol: SymbolId,
}

impl SymbolIdentity {
    #[must_use]
    pub const fn new(bubble: BubbleId, symbol: SymbolId) -> Self {
        Self { bubble, symbol }
    }

    #[must_use]
    pub const fn bubble(self) -> BubbleId {
        self.bubble
    }

    #[must_use]
    pub const fn symbol(self) -> SymbolId {
        self.symbol
    }
}

/// A UTF-8 byte offset in a source file.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct TextSize(u32);

impl TextSize {
    #[must_use]
    pub const fn from_u32(value: u32) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn to_u32(self) -> u32 {
        self.0
    }

    #[must_use]
    pub fn try_from_usize(value: usize) -> Option<Self> {
        u32::try_from(value).ok().map(Self)
    }

    #[must_use]
    pub fn to_usize(self) -> usize {
        self.0 as usize
    }
}

/// A validated half-open byte range.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct TextRange {
    start: TextSize,
    end: TextSize,
}

impl TextRange {
    #[must_use]
    pub const fn new(start: TextSize, end: TextSize) -> Option<Self> {
        if start.0 <= end.0 {
            Some(Self { start, end })
        } else {
            None
        }
    }

    #[must_use]
    pub const fn empty(offset: TextSize) -> Self {
        Self {
            start: offset,
            end: offset,
        }
    }

    #[must_use]
    pub const fn start(self) -> TextSize {
        self.start
    }

    #[must_use]
    pub const fn end(self) -> TextSize {
        self.end
    }

    #[must_use]
    pub const fn len(self) -> TextSize {
        TextSize(self.end.0 - self.start.0)
    }

    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.start.0 == self.end.0
    }
}

/// A source location with an optional generated-origin link.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct SourceSpan {
    file: FileId,
    range: TextRange,
    origin: Option<SpanId>,
}

impl SourceSpan {
    #[must_use]
    pub const fn new(file: FileId, range: TextRange) -> Self {
        Self {
            file,
            range,
            origin: None,
        }
    }

    #[must_use]
    pub const fn with_origin(mut self, origin: SpanId) -> Self {
        self.origin = Some(origin);
        self
    }

    #[must_use]
    pub const fn file(self) -> FileId {
        self.file
    }

    #[must_use]
    pub const fn range(self) -> TextRange {
        self.range
    }

    #[must_use]
    pub const fn origin(self) -> Option<SpanId> {
        self.origin
    }
}

/// Fixed FNV-1a hashing for deterministic compiler keys and test baselines.
#[must_use]
pub const fn stable_hash_bytes(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325;
    let mut index = 0;
    while index < bytes.len() {
        hash ^= bytes[index] as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        index += 1;
    }
    hash
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct DiagnosticCode(&'static str);

impl DiagnosticCode {
    /// Creates a statically known Pop Lang diagnostic code.
    ///
    /// # Panics
    ///
    /// Panics when `value` is not `POP` followed by exactly four ASCII digits.
    #[must_use]
    pub const fn new(value: &'static str) -> Self {
        let bytes = value.as_bytes();
        assert!(
            bytes.len() == 7,
            "diagnostic code must be POP followed by four digits"
        );
        assert!(bytes[0] == b'P' && bytes[1] == b'O' && bytes[2] == b'P');
        let mut index = 3;
        while index < 7 {
            assert!(bytes[index] >= b'0' && bytes[index] <= b'9');
            index += 1;
        }
        Self(value)
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        self.0
    }
}

impl fmt::Display for DiagnosticCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.0)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DiagnosticSeverity {
    Error,
    Warning,
    Information,
    Hint,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DiagnosticCategory {
    Syntax,
    Resolution,
    Type,
    Flow,
    CompileTime,
    RuntimeSafety,
    Style,
    Backend,
    Project,
    Tooling,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct MessageKey(&'static str);

impl MessageKey {
    #[must_use]
    pub const fn new(value: &'static str) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        self.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DiagnosticArgument {
    Character(char),
    Identifier(String),
    Unsigned(u64),
    SyntaxExpectation(&'static str),
    Token(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiagnosticLabel {
    span: SourceSpan,
    message_key: MessageKey,
    arguments: Vec<DiagnosticArgument>,
}

impl DiagnosticLabel {
    #[must_use]
    pub fn new(
        span: SourceSpan,
        message_key: MessageKey,
        arguments: Vec<DiagnosticArgument>,
    ) -> Self {
        Self {
            span,
            message_key,
            arguments,
        }
    }

    #[must_use]
    pub const fn span(&self) -> SourceSpan {
        self.span
    }

    #[must_use]
    pub const fn message_key(&self) -> MessageKey {
        self.message_key
    }

    #[must_use]
    pub fn arguments(&self) -> &[DiagnosticArgument] {
        &self.arguments
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiagnosticNote {
    message_key: MessageKey,
    arguments: Vec<DiagnosticArgument>,
}

impl DiagnosticNote {
    #[must_use]
    pub fn new(message_key: MessageKey, arguments: Vec<DiagnosticArgument>) -> Self {
        Self {
            message_key,
            arguments,
        }
    }

    #[must_use]
    pub const fn message_key(&self) -> MessageKey {
        self.message_key
    }

    #[must_use]
    pub fn arguments(&self) -> &[DiagnosticArgument] {
        &self.arguments
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DiagnosticOriginKind {
    Source,
    Generated,
    Desugared,
    CompileTime,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DiagnosticOrigin {
    span: SourceSpan,
    kind: DiagnosticOriginKind,
}

impl DiagnosticOrigin {
    #[must_use]
    pub const fn new(span: SourceSpan, kind: DiagnosticOriginKind) -> Self {
        Self { span, kind }
    }

    #[must_use]
    pub const fn span(self) -> SourceSpan {
        self.span
    }

    #[must_use]
    pub const fn kind(self) -> DiagnosticOriginKind {
        self.kind
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WarningWave(u32);

impl WarningWave {
    #[must_use]
    pub const fn new(value: u32) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn value(self) -> u32 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SuppressionKey(&'static str);

impl SuppressionKey {
    #[must_use]
    pub const fn new(value: &'static str) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FixApplicability {
    Safe,
    RequiresReview,
    Unsafe,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TextEdit {
    file: FileId,
    range: TextRange,
    replacement: String,
}

impl TextEdit {
    #[must_use]
    pub fn new(file: FileId, range: TextRange, replacement: impl Into<String>) -> Self {
        Self {
            file,
            range,
            replacement: replacement.into(),
        }
    }

    #[must_use]
    pub const fn file(&self) -> FileId {
        self.file
    }

    #[must_use]
    pub const fn range(&self) -> TextRange {
        self.range
    }

    #[must_use]
    pub fn replacement(&self) -> &str {
        &self.replacement
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkspaceEdit {
    revision: u64,
    edits: Vec<TextEdit>,
}

impl WorkspaceEdit {
    #[must_use]
    pub fn new(revision: u64, edits: Vec<TextEdit>) -> Self {
        Self { revision, edits }
    }

    #[must_use]
    pub const fn revision(&self) -> u64 {
        self.revision
    }

    #[must_use]
    pub fn edits(&self) -> &[TextEdit] {
        &self.edits
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QuickFix {
    id: &'static str,
    title_key: MessageKey,
    applicability: FixApplicability,
    edit: WorkspaceEdit,
}

impl QuickFix {
    #[must_use]
    pub const fn new(
        id: &'static str,
        title_key: MessageKey,
        applicability: FixApplicability,
        edit: WorkspaceEdit,
    ) -> Self {
        Self {
            id,
            title_key,
            applicability,
            edit,
        }
    }

    #[must_use]
    pub const fn id(&self) -> &'static str {
        self.id
    }

    #[must_use]
    pub const fn title_key(&self) -> MessageKey {
        self.title_key
    }

    #[must_use]
    pub const fn is_safe(&self) -> bool {
        matches!(self.applicability, FixApplicability::Safe)
    }

    #[must_use]
    pub const fn edit(&self) -> &WorkspaceEdit {
        &self.edit
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Diagnostic {
    code: DiagnosticCode,
    severity: DiagnosticSeverity,
    category: DiagnosticCategory,
    message_key: MessageKey,
    arguments: Vec<DiagnosticArgument>,
    primary_span: SourceSpan,
    labels: Vec<DiagnosticLabel>,
    notes: Vec<DiagnosticNote>,
    origin_chain: Vec<DiagnosticOrigin>,
    fixes: Vec<QuickFix>,
    warning_wave: Option<WarningWave>,
    suppression_key: Option<SuppressionKey>,
}

impl Diagnostic {
    #[must_use]
    pub fn new(
        code: DiagnosticCode,
        severity: DiagnosticSeverity,
        category: DiagnosticCategory,
        message_key: MessageKey,
        arguments: Vec<DiagnosticArgument>,
        primary_span: SourceSpan,
    ) -> Self {
        Self {
            code,
            severity,
            category,
            message_key,
            arguments,
            primary_span,
            labels: Vec::new(),
            notes: Vec::new(),
            origin_chain: Vec::new(),
            fixes: Vec::new(),
            warning_wave: None,
            suppression_key: None,
        }
    }

    #[must_use]
    pub fn with_label(mut self, label: DiagnosticLabel) -> Self {
        self.labels.push(label);
        self
    }

    #[must_use]
    pub fn with_note(mut self, note: DiagnosticNote) -> Self {
        self.notes.push(note);
        self
    }

    #[must_use]
    pub fn with_origin(mut self, origin: DiagnosticOrigin) -> Self {
        self.origin_chain.push(origin);
        self
    }

    #[must_use]
    pub fn with_fix(mut self, fix: QuickFix) -> Self {
        self.fixes.push(fix);
        self
    }

    #[must_use]
    pub const fn with_warning_wave(mut self, warning_wave: WarningWave) -> Self {
        self.warning_wave = Some(warning_wave);
        self
    }

    #[must_use]
    pub const fn with_suppression_key(mut self, suppression_key: SuppressionKey) -> Self {
        self.suppression_key = Some(suppression_key);
        self
    }

    #[must_use]
    pub const fn code(&self) -> DiagnosticCode {
        self.code
    }

    #[must_use]
    pub const fn severity(&self) -> DiagnosticSeverity {
        self.severity
    }

    #[must_use]
    pub const fn category(&self) -> DiagnosticCategory {
        self.category
    }

    #[must_use]
    pub const fn message_key(&self) -> MessageKey {
        self.message_key
    }

    #[must_use]
    pub fn arguments(&self) -> &[DiagnosticArgument] {
        &self.arguments
    }

    #[must_use]
    pub const fn primary_span(&self) -> SourceSpan {
        self.primary_span
    }

    #[must_use]
    pub fn labels(&self) -> &[DiagnosticLabel] {
        &self.labels
    }

    #[must_use]
    pub fn notes(&self) -> &[DiagnosticNote] {
        &self.notes
    }

    #[must_use]
    pub fn origin_chain(&self) -> &[DiagnosticOrigin] {
        &self.origin_chain
    }

    #[must_use]
    pub fn fixes(&self) -> &[QuickFix] {
        &self.fixes
    }

    #[must_use]
    pub const fn warning_wave(&self) -> Option<WarningWave> {
        self.warning_wave
    }

    #[must_use]
    pub const fn suppression_key(&self) -> Option<SuppressionKey> {
        self.suppression_key
    }
}
