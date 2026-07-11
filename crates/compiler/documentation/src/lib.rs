//! Safe XML documentation parsing and declaration attachment.

use pop_diagnostics::documentation as documentation_diagnostics;
use pop_foundation::{Diagnostic, SourceSpan, TextRange};
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
    blocks: Vec<DocumentationBlock>,
    diagnostics: Vec<Diagnostic>,
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
    let Some(documentation_line) = source.line_column(documentation_range.end()) else {
        return false;
    };
    let Some(target_line) = source.line_column(target.range().start()) else {
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
    fn parse(text: &str) -> Result<Self, XmlParseError> {
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
enum XmlParseError {
    UnsafeConstruct,
    Malformed,
}

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
