//! Safe XML documentation parsing and declaration attachment.

use std::collections::BTreeSet;

use pop_diagnostics::documentation as documentation_diagnostics;
use pop_foundation::{Diagnostic, FileId, SourceSpan, TextRange};
use pop_source::SourceFile;
use pop_syntax::{NodeKind, SyntaxNode, SyntaxTree, TokenKind};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DocumentationTarget {
    kind: NodeKind,
    range: TextRange,
}

impl DocumentationTarget {
    #[must_use]
    pub const fn kind(&self) -> NodeKind {
        self.kind
    }

    #[must_use]
    pub const fn range(&self) -> TextRange {
        self.range
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DocumentationBlock {
    range: TextRange,
    xml_text: String,
    xml: Option<XmlFragment>,
    target: Option<DocumentationTarget>,
}

impl DocumentationBlock {
    #[must_use]
    pub const fn range(&self) -> TextRange {
        self.range
    }

    #[must_use]
    pub fn xml_text(&self) -> &str {
        &self.xml_text
    }

    #[must_use]
    pub const fn xml(&self) -> Option<&XmlFragment> {
        self.xml.as_ref()
    }

    #[must_use]
    pub const fn target(&self) -> Option<&DocumentationTarget> {
        self.target.as_ref()
    }
}

#[derive(Clone, Debug)]
pub struct DocumentationAnalysis {
    source: SourceFile,
    file: FileId,
    blocks: Vec<DocumentationBlock>,
    diagnostics: Vec<Diagnostic>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TypedErrorDocumentationContract {
    target: TextRange,
    error_names: Vec<String>,
    cases: Vec<String>,
    inherited_error_tags: Vec<String>,
    require_complete: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PublicErrorDocumentationContract {
    declaration: TextRange,
    name: String,
    cases: Vec<(String, TextRange)>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DocumentedReturnKind {
    None,
    Values,
    ResultOk,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TypedReturnsDocumentationContract {
    target: TextRange,
    kind: DocumentedReturnKind,
}

impl TypedReturnsDocumentationContract {
    #[must_use]
    pub const fn without_result(target: TextRange) -> Self {
        Self {
            target,
            kind: DocumentedReturnKind::None,
        }
    }

    #[must_use]
    pub const fn values(target: TextRange) -> Self {
        Self {
            target,
            kind: DocumentedReturnKind::Values,
        }
    }

    #[must_use]
    pub const fn result_ok(target: TextRange) -> Self {
        Self {
            target,
            kind: DocumentedReturnKind::ResultOk,
        }
    }
}

impl PublicErrorDocumentationContract {
    #[must_use]
    pub fn new<I, S>(declaration: TextRange, name: impl Into<String>, cases: I) -> Self
    where
        I: IntoIterator<Item = (S, TextRange)>,
        S: Into<String>,
    {
        Self {
            declaration,
            name: name.into(),
            cases: cases
                .into_iter()
                .map(|(name, range)| (name.into(), range))
                .collect(),
        }
    }
}

impl TypedErrorDocumentationContract {
    #[must_use]
    pub fn result<I, S>(
        target: TextRange,
        error_name: impl Into<String>,
        cases: I,
        require_complete: bool,
    ) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            target,
            error_names: vec![error_name.into()],
            cases: cases.into_iter().map(Into::into).collect(),
            inherited_error_tags: Vec::new(),
            require_complete,
        }
    }

    #[must_use]
    pub const fn without_result(target: TextRange) -> Self {
        Self {
            target,
            error_names: Vec::new(),
            cases: Vec::new(),
            inherited_error_tags: Vec::new(),
            require_complete: false,
        }
    }

    #[must_use]
    pub fn result_with_names<I, S, C, T>(
        target: TextRange,
        error_names: I,
        cases: C,
        require_complete: bool,
    ) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
        C: IntoIterator<Item = T>,
        T: Into<String>,
    {
        Self {
            target,
            error_names: error_names.into_iter().map(Into::into).collect(),
            cases: cases.into_iter().map(Into::into).collect(),
            inherited_error_tags: Vec::new(),
            require_complete,
        }
    }

    #[must_use]
    pub fn with_inherited_error_tags<I, S>(mut self, tags: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.inherited_error_tags = tags.into_iter().map(Into::into).collect();
        self
    }
}

impl DocumentationAnalysis {
    #[must_use]
    pub fn analyze(source: &SourceFile, syntax: &SyntaxTree) -> Self {
        let mut blocks = Vec::new();
        let mut diagnostics = Vec::new();
        let tokens = syntax.tokens();
        let mut cursor = 0;

        while cursor < tokens.len() {
            if tokens[cursor].kind() != TokenKind::DocumentationComment {
                cursor += 1;
                continue;
            }

            let first = cursor;
            let mut last = cursor;
            let mut lines = vec![documentation_line(tokens[cursor].text(source))];
            while let Some(next) = next_documentation_line(tokens, last) {
                last = next;
                lines.push(documentation_line(tokens[last].text(source)));
            }

            let range = TextRange::new(tokens[first].range().start(), tokens[last].range().end())
                .unwrap_or_else(|| TextRange::empty(tokens[first].range().start()));
            let xml_text = lines.join("\n");
            let span = SourceSpan::new(source.id(), range);
            let xml = match XmlFragment::parse(&xml_text) {
                Ok(xml) => Some(xml),
                Err(XmlParseError::UnsafeConstruct) => {
                    diagnostics.push(documentation_diagnostics::unsafe_xml(span));
                    None
                }
                Err(XmlParseError::Malformed) => {
                    diagnostics.push(documentation_diagnostics::malformed_xml(span));
                    None
                }
            };
            let target = find_target(source, syntax.root().children(), range);
            blocks.push(DocumentationBlock {
                range,
                xml_text,
                xml,
                target,
            });
            cursor = last + 1;
        }

        diagnostics.sort_by_key(|diagnostic| diagnostic.primary_span().range().start());
        Self {
            source: source.clone(),
            file: source.id(),
            blocks,
            diagnostics,
        }
    }

    #[must_use]
    pub fn blocks(&self) -> &[DocumentationBlock] {
        &self.blocks
    }

    #[must_use]
    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    #[must_use]
    pub fn diagnostic_snapshot(&self) -> String {
        let mut snapshot = String::new();
        for diagnostic in &self.diagnostics {
            let range = diagnostic.primary_span().range();
            snapshot.push_str(diagnostic.code().as_str());
            snapshot.push('@');
            snapshot.push_str(&range.start().to_u32().to_string());
            snapshot.push_str("..");
            snapshot.push_str(&range.end().to_u32().to_string());
            snapshot.push('\n');
        }
        snapshot
    }

    /// Checks `<error>` tags against exact nominal result-error identities.
    ///
    /// Contracts are produced by semantic analysis; this phase never resolves
    /// names dynamically and emits no runtime metadata.
    pub fn validate_typed_errors(&mut self, contracts: &[TypedErrorDocumentationContract]) {
        let mut diagnostics = Vec::new();
        for contract in contracts {
            let block = self.blocks.iter().find(|block| {
                block
                    .target()
                    .is_some_and(|target| target.range() == contract.target)
            });
            let span = SourceSpan::new(
                self.file,
                block.map_or(contract.target, DocumentationBlock::range),
            );
            let mut tags = block
                .and_then(DocumentationBlock::xml)
                .map(error_tag_types)
                .unwrap_or_default();
            let local_tags: BTreeSet<_> = tags.iter().cloned().collect();
            tags.extend(
                contract
                    .inherited_error_tags
                    .iter()
                    .filter(|tag| !local_tags.contains(*tag))
                    .cloned(),
            );
            let Some(canonical_error_name) = contract.error_names.first() else {
                for tag in tags {
                    diagnostics.push(documentation_diagnostics::invalid_error_tag(span, tag));
                }
                continue;
            };

            let mut seen = BTreeSet::new();
            let mut covered_cases = BTreeSet::new();
            let mut covers_all = false;
            for tag in tags {
                if !seen.insert(tag.clone()) {
                    diagnostics.push(documentation_diagnostics::invalid_error_tag(span, tag));
                    continue;
                }
                if contract.error_names.iter().any(|name| tag == *name) {
                    covers_all = true;
                    continue;
                }
                let Some(case) = contract.error_names.iter().find_map(|name| {
                    tag.strip_prefix(name)
                        .and_then(|rest| rest.strip_prefix('.'))
                }) else {
                    diagnostics.push(documentation_diagnostics::invalid_error_tag(span, tag));
                    continue;
                };
                if contract.cases.iter().any(|candidate| candidate == case) {
                    covered_cases.insert(case.to_owned());
                } else {
                    diagnostics.push(documentation_diagnostics::invalid_error_tag(span, tag));
                }
            }
            if contract.require_complete && !covers_all {
                for case in &contract.cases {
                    if !covered_cases.contains(case) {
                        diagnostics.push(documentation_diagnostics::missing_error_case(
                            span,
                            format!("{canonical_error_name}.{case}"),
                        ));
                    }
                }
            }
        }
        self.diagnostics.extend(diagnostics);
        self.diagnostics
            .sort_by_key(|diagnostic| diagnostic.primary_span().range().start());
    }

    /// Enforces ADR 0052's checked summary contract for public nominal errors.
    pub fn validate_public_error_summaries(
        &mut self,
        contracts: &[PublicErrorDocumentationContract],
    ) {
        let mut diagnostics = Vec::new();
        for contract in contracts {
            self.validate_summary_target(
                contract.declaration,
                contract.name.clone(),
                &mut diagnostics,
            );
            for (case, range) in &contract.cases {
                self.validate_summary_target(
                    *range,
                    format!("{}.{}", contract.name, case),
                    &mut diagnostics,
                );
            }
        }
        self.diagnostics.extend(diagnostics);
        self.diagnostics.sort_by_key(|diagnostic| {
            (
                diagnostic.primary_span().range().start(),
                diagnostic.code().as_str(),
            )
        });
    }

    /// Checks `<returns>` against the typed function result contract.
    pub fn validate_typed_returns(&mut self, contracts: &[TypedReturnsDocumentationContract]) {
        let mut diagnostics = Vec::new();
        for contract in contracts {
            let Some(block) = self.block_attached_to_range(contract.target) else {
                continue;
            };
            let Some(xml) = block.xml() else {
                continue;
            };
            let count = element_count(xml, "returns");
            let valid = match contract.kind {
                DocumentedReturnKind::None => count == 0,
                DocumentedReturnKind::Values | DocumentedReturnKind::ResultOk => count <= 1,
            };
            if !valid {
                let expectation = match contract.kind {
                    DocumentedReturnKind::None => "no result",
                    DocumentedReturnKind::Values => "function result",
                    DocumentedReturnKind::ResultOk => "Result.Ok value",
                };
                diagnostics.push(documentation_diagnostics::invalid_returns(
                    SourceSpan::new(self.file, block.range()),
                    expectation,
                ));
            }
        }
        self.diagnostics.extend(diagnostics);
        self.diagnostics.sort_by_key(|diagnostic| {
            (
                diagnostic.primary_span().range().start(),
                diagnostic.code().as_str(),
            )
        });
    }

    fn validate_summary_target(
        &self,
        target: TextRange,
        name: String,
        diagnostics: &mut Vec<Diagnostic>,
    ) {
        let block = self.block_attached_to_range(target);
        let Some(count) = block.and_then(|block| block.xml().map(summary_count)) else {
            if block.is_none() {
                diagnostics.push(documentation_diagnostics::missing_summary(
                    SourceSpan::new(self.file, target),
                    name,
                ));
            }
            return;
        };
        let span = SourceSpan::new(self.file, block.map_or(target, DocumentationBlock::range));
        if count == 0 {
            diagnostics.push(documentation_diagnostics::missing_summary(span, name));
        } else if count > 1 {
            diagnostics.push(documentation_diagnostics::duplicate_summary(span, name));
        }
    }

    fn block_attached_to_range(&self, target: TextRange) -> Option<&DocumentationBlock> {
        self.blocks
            .iter()
            .find(|block| {
                block
                    .target()
                    .is_some_and(|candidate| candidate.range() == target)
            })
            .or_else(|| {
                self.blocks.iter().rev().find(|block| {
                    block.range().end() <= target.start()
                        && attachment_gap_is_valid_range(&self.source, block.range(), target)
                })
            })
    }

    #[must_use]
    pub fn error_tags_for_target(&self, target: TextRange) -> Vec<String> {
        self.block_attached_to_range(target)
            .and_then(DocumentationBlock::xml)
            .map(error_tag_types)
            .unwrap_or_default()
    }

    #[must_use]
    pub fn inheritance_references_for_target(&self, target: TextRange) -> Vec<String> {
        self.block_attached_to_range(target)
            .and_then(DocumentationBlock::xml)
            .map(inheritance_references)
            .unwrap_or_default()
    }

    #[must_use]
    pub fn xml_for_target(&self, target: TextRange) -> Option<&XmlFragment> {
        self.block_attached_to_range(target)
            .and_then(DocumentationBlock::xml)
    }
}

fn summary_count(xml: &XmlFragment) -> usize {
    element_count(xml, "summary")
}

fn element_count(xml: &XmlFragment, element: &str) -> usize {
    xml.children()
        .iter()
        .filter(|node| matches!(node, XmlNode::Element { name, .. } if name == element))
        .count()
}

fn error_tag_types(xml: &XmlFragment) -> Vec<String> {
    xml.children()
        .iter()
        .filter_map(|node| match node {
            XmlNode::Element {
                name, attributes, ..
            } if name == "error" => attributes
                .iter()
                .find(|attribute| attribute.name() == "type")
                .map(|attribute| attribute.value().to_owned())
                .or_else(|| Some("<missing>".to_owned())),
            _ => None,
        })
        .collect()
}

fn inheritance_references(xml: &XmlFragment) -> Vec<String> {
    xml.children()
        .iter()
        .filter_map(|node| match node {
            XmlNode::Element {
                name, attributes, ..
            } if name == "inheritdoc" => attributes
                .iter()
                .find(|attribute| attribute.name() == "cref")
                .map(|attribute| attribute.value().to_owned())
                .or_else(|| Some("<missing>".to_owned())),
            _ => None,
        })
        .collect()
}

fn next_documentation_line(tokens: &[pop_syntax::Token], current: usize) -> Option<usize> {
    let mut cursor = current + 1;
    if tokens.get(cursor)?.kind() != TokenKind::Newline {
        return None;
    }
    cursor += 1;
    if tokens
        .get(cursor)
        .is_some_and(|token| token.kind() == TokenKind::Whitespace)
    {
        cursor += 1;
    }
    (tokens.get(cursor)?.kind() == TokenKind::DocumentationComment).then_some(cursor)
}

fn documentation_line(comment: &str) -> String {
    comment
        .strip_prefix("---")
        .unwrap_or(comment)
        .strip_prefix(' ')
        .unwrap_or_else(|| comment.strip_prefix("---").unwrap_or(comment))
        .to_owned()
}

fn find_target(
    source: &SourceFile,
    nodes: &[SyntaxNode],
    documentation_range: TextRange,
) -> Option<DocumentationTarget> {
    let mut candidates = nodes
        .iter()
        .filter(|node| node.range().start() >= documentation_range.end());
    let mut target = candidates.next()?;
    while target.kind() == NodeKind::AttributeUse {
        target = candidates.next()?;
    }
    if !is_documentable(target.kind())
        || !attachment_gap_is_valid(source, documentation_range, target)
    {
        return None;
    }
    Some(DocumentationTarget {
        kind: target.kind(),
        range: target.range(),
    })
}

fn attachment_gap_is_valid(
    source: &SourceFile,
    documentation_range: TextRange,
    target: &SyntaxNode,
) -> bool {
    attachment_gap_is_valid_range(source, documentation_range, target.range())
}

fn attachment_gap_is_valid_range(
    source: &SourceFile,
    documentation_range: TextRange,
    target: TextRange,
) -> bool {
    let Some(documentation_line) = source.line_column(documentation_range.end()) else {
        return false;
    };
    let Some(target_line) = source.line_column(target.start()) else {
        return false;
    };
    let source_lines: Vec<_> = source.text().lines().collect();
    for line_number in documentation_line.line() + 1..target_line.line() {
        let Some(line) = source_lines.get(line_number as usize) else {
            return false;
        };
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with("--") || !trimmed.starts_with('@') {
            return false;
        }
    }
    true
}

const fn is_documentable(kind: NodeKind) -> bool {
    matches!(
        kind,
        NodeKind::NamespaceDeclaration
            | NodeKind::FunctionDeclaration
            | NodeKind::ConstDeclaration
            | NodeKind::TypeAliasDeclaration
            | NodeKind::AttributeDeclaration
            | NodeKind::RecordDeclaration
            | NodeKind::UnionDeclaration
            | NodeKind::ErrorDeclaration
            | NodeKind::ClassDeclaration
            | NodeKind::InterfaceDeclaration
            | NodeKind::EnumDeclaration
    )
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct XmlFragment {
    children: Vec<XmlNode>,
}

impl XmlFragment {
    /// Parses one safe, entity-limited XML fragment for documentation tools.
    ///
    /// # Errors
    ///
    /// Rejects malformed XML and every DTD, processing-instruction, or other
    /// unsafe construct excluded by ADR 0014.
    pub fn parse(text: &str) -> Result<Self, XmlParseError> {
        let mut parser = XmlParser { text, cursor: 0 };
        let children = parser.parse_children(None)?;
        Ok(Self { children })
    }

    #[must_use]
    pub fn children(&self) -> &[XmlNode] {
        &self.children
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum XmlNode {
    Element {
        name: String,
        attributes: Vec<XmlAttribute>,
        children: Vec<XmlNode>,
    },
    Text(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct XmlAttribute {
    name: String,
    value: String,
}

impl XmlAttribute {
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub fn value(&self) -> &str {
        &self.value
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum XmlParseError {
    UnsafeConstruct,
    Malformed,
}

impl std::fmt::Display for XmlParseError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsafeConstruct => formatter.write_str("unsafe XML documentation construct"),
            Self::Malformed => formatter.write_str("malformed XML documentation fragment"),
        }
    }
}

impl std::error::Error for XmlParseError {}

struct XmlParser<'text> {
    text: &'text str,
    cursor: usize,
}

impl XmlParser<'_> {
    fn parse_children(&mut self, closing: Option<&str>) -> Result<Vec<XmlNode>, XmlParseError> {
        let mut children = Vec::new();
        loop {
            if self.cursor == self.text.len() {
                return if closing.is_none() {
                    Ok(children)
                } else {
                    Err(XmlParseError::Malformed)
                };
            }
            if self.remaining().starts_with("</") {
                let expected = closing.ok_or(XmlParseError::Malformed)?;
                self.cursor += 2;
                let actual = self.parse_name()?.to_owned();
                self.skip_whitespace();
                self.consume(">")?;
                return if actual == expected {
                    Ok(children)
                } else {
                    Err(XmlParseError::Malformed)
                };
            }
            if self.remaining().starts_with("<!") || self.remaining().starts_with("<?") {
                return Err(XmlParseError::UnsafeConstruct);
            }
            if self.remaining().starts_with('<') {
                children.push(self.parse_element()?);
            } else {
                let text = self.parse_text()?;
                if !text.trim().is_empty() {
                    children.push(XmlNode::Text(text));
                }
            }
        }
    }

    fn parse_element(&mut self) -> Result<XmlNode, XmlParseError> {
        self.consume("<")?;
        let name = self.parse_name()?.to_owned();
        let mut attributes = Vec::new();
        loop {
            self.skip_whitespace();
            if self.remaining().starts_with("/>") {
                self.cursor += 2;
                return Ok(XmlNode::Element {
                    name,
                    attributes,
                    children: Vec::new(),
                });
            }
            if self.remaining().starts_with('>') {
                self.cursor += 1;
                let children = self.parse_children(Some(&name))?;
                return Ok(XmlNode::Element {
                    name,
                    attributes,
                    children,
                });
            }
            let attribute_name = self.parse_name()?.to_owned();
            self.skip_whitespace();
            self.consume("=")?;
            self.skip_whitespace();
            let value = self.parse_quoted_value()?;
            attributes.push(XmlAttribute {
                name: attribute_name,
                value,
            });
        }
    }

    fn parse_name(&mut self) -> Result<&str, XmlParseError> {
        let start = self.cursor;
        while let Some(character) = self.remaining().chars().next() {
            if !(character.is_alphanumeric() || matches!(character, '_' | '-' | ':' | '.')) {
                break;
            }
            self.cursor += character.len_utf8();
        }
        if self.cursor == start {
            Err(XmlParseError::Malformed)
        } else {
            Ok(&self.text[start..self.cursor])
        }
    }

    fn parse_quoted_value(&mut self) -> Result<String, XmlParseError> {
        let quote = self
            .remaining()
            .chars()
            .next()
            .ok_or(XmlParseError::Malformed)?;
        if !matches!(quote, '\'' | '"') {
            return Err(XmlParseError::Malformed);
        }
        self.cursor += quote.len_utf8();
        let start = self.cursor;
        while let Some(character) = self.remaining().chars().next() {
            if character == quote {
                let value = self.text[start..self.cursor].to_owned();
                validate_entities(&value)?;
                self.cursor += quote.len_utf8();
                return Ok(value);
            }
            if character == '<' {
                return Err(XmlParseError::Malformed);
            }
            self.cursor += character.len_utf8();
        }
        Err(XmlParseError::Malformed)
    }

    fn parse_text(&mut self) -> Result<String, XmlParseError> {
        let start = self.cursor;
        while let Some(character) = self.remaining().chars().next() {
            if character == '<' {
                break;
            }
            self.cursor += character.len_utf8();
        }
        let text = self.text[start..self.cursor].to_owned();
        validate_entities(&text)?;
        Ok(text)
    }

    fn skip_whitespace(&mut self) {
        while let Some(character) = self.remaining().chars().next() {
            if !character.is_whitespace() {
                break;
            }
            self.cursor += character.len_utf8();
        }
    }

    fn consume(&mut self, expected: &str) -> Result<(), XmlParseError> {
        if self.remaining().starts_with(expected) {
            self.cursor += expected.len();
            Ok(())
        } else {
            Err(XmlParseError::Malformed)
        }
    }

    fn remaining(&self) -> &str {
        &self.text[self.cursor..]
    }
}

fn validate_entities(text: &str) -> Result<(), XmlParseError> {
    let mut remaining = text;
    while let Some(start) = remaining.find('&') {
        remaining = &remaining[start + 1..];
        let Some(end) = remaining.find(';') else {
            return Err(XmlParseError::Malformed);
        };
        let entity = &remaining[..end];
        let numeric = entity
            .strip_prefix('#')
            .is_some_and(|number| number.parse::<u32>().is_ok())
            || entity
                .strip_prefix("#x")
                .is_some_and(|number| u32::from_str_radix(number, 16).is_ok());
        if !numeric && !matches!(entity, "amp" | "lt" | "gt" | "quot" | "apos") {
            return Err(XmlParseError::UnsafeConstruct);
        }
        remaining = &remaining[end + 1..];
    }
    Ok(())
}
