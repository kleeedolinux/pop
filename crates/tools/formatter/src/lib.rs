//! Canonical formatter over the lossless syntax tree.

use pop_documentation::XmlFragment;
use pop_source::SourceFile;
use pop_syntax::{TokenKind, lex};

/// Formats canonical multiline XML documentation comments while preserving all
/// non-documentation source bytes.
///
/// Well-formed non-empty elements written on one documentation line are
/// expanded to separate opening, content, and closing lines. Adjacent top-level
/// elements receive one empty documentation line. Malformed XML and ordinary
/// comments are preserved for diagnostic recovery.
#[must_use]
pub fn format_documentation_comments(source: &SourceFile) -> String {
    let text = source.text();
    let newline = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let lexed = lex(source);
    let mut output = String::with_capacity(text.len());
    let mut cursor = 0;
    let mut state = DocumentationBlockState::default();
    let mut previous_documentation_end = None;

    for token in lexed
        .tokens()
        .iter()
        .copied()
        .filter(|token| token.kind() == TokenKind::DocumentationComment)
    {
        let start = token.range().start().to_usize();
        let end = token.range().end().to_usize();
        let indentation = line_indentation(text, start);
        let gap = &text[cursor..start];
        let adjacent = previous_documentation_end
            .is_some_and(|previous| previous == cursor && is_adjacent_gap(gap, indentation));
        if !adjacent {
            state = DocumentationBlockState::default();
        }

        let comment = token.text(source);
        let shape = documentation_shape(comment);
        let needs_separator = adjacent && state.needs_separator_before(shape);
        if needs_separator {
            output.push_str(newline);
            output.push_str(indentation);
            output.push_str("---");
            output.push_str(newline);
            output.push_str(indentation);
        } else {
            output.push_str(gap);
        }

        if let Some(element) = inline_element(comment) {
            output.push_str("--- ");
            output.push_str(element.opening);
            output.push_str(newline);
            output.push_str(indentation);
            if !element.content.is_empty() {
                output.push_str("--- ");
                output.push_str(element.content);
                output.push_str(newline);
                output.push_str(indentation);
            }
            output.push_str("--- ");
            output.push_str(element.closing);
            if comment.ends_with('\r') {
                output.push('\r');
            }
        } else {
            output.push_str(comment);
        }

        state.observe(shape);
        cursor = end;
        previous_documentation_end = Some(end);
    }

    output.push_str(&text[cursor..]);
    output
}

#[derive(Clone, Copy, Debug, Default)]
struct DocumentationBlockState {
    depth: usize,
    completed_top_level: bool,
    has_separator: bool,
}

impl DocumentationBlockState {
    const fn needs_separator_before(self, shape: DocumentationShape) -> bool {
        self.depth == 0
            && self.completed_top_level
            && !self.has_separator
            && shape.starts_top_level_element()
    }

    fn observe(&mut self, shape: DocumentationShape) {
        match shape {
            DocumentationShape::Blank => self.has_separator = true,
            DocumentationShape::Inline | DocumentationShape::SelfClosing if self.depth == 0 => {
                self.completed_top_level = true;
                self.has_separator = false;
            }
            DocumentationShape::Opening if self.depth == 0 => {
                self.depth = 1;
                self.completed_top_level = false;
                self.has_separator = false;
            }
            DocumentationShape::Opening => self.depth += 1,
            DocumentationShape::Closing if self.depth > 1 => self.depth -= 1,
            DocumentationShape::Closing if self.depth == 1 => {
                self.depth = 0;
                self.completed_top_level = true;
                self.has_separator = false;
            }
            DocumentationShape::Unknown => {
                self.completed_top_level = false;
                self.has_separator = false;
            }
            DocumentationShape::Content
            | DocumentationShape::Closing
            | DocumentationShape::Inline
            | DocumentationShape::SelfClosing => {}
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DocumentationShape {
    Blank,
    Content,
    Inline,
    Opening,
    Closing,
    SelfClosing,
    Unknown,
}

impl DocumentationShape {
    const fn starts_top_level_element(self) -> bool {
        matches!(self, Self::Inline | Self::Opening | Self::SelfClosing)
    }
}

#[derive(Clone, Copy, Debug)]
struct InlineElement<'source> {
    opening: &'source str,
    content: &'source str,
    closing: &'source str,
}

fn documentation_shape(comment: &str) -> DocumentationShape {
    let body = documentation_body(comment);
    if body.is_empty() {
        return DocumentationShape::Blank;
    }
    if inline_element(comment).is_some() {
        return DocumentationShape::Inline;
    }
    if body.starts_with("</") && body.ends_with('>') && valid_closing_tag(body) {
        return DocumentationShape::Closing;
    }
    if body.starts_with('<') && body.ends_with("/>") {
        return DocumentationShape::SelfClosing;
    }
    if body.starts_with('<')
        && body.ends_with('>')
        && !body.starts_with("</")
        && !body[1..body.len() - 1].contains('<')
    {
        return DocumentationShape::Opening;
    }
    if body.starts_with('<') {
        DocumentationShape::Unknown
    } else {
        DocumentationShape::Content
    }
}

fn inline_element(comment: &str) -> Option<InlineElement<'_>> {
    let body = documentation_body(comment);
    if !body.starts_with('<') || body.starts_with("</") || body.ends_with("/>") {
        return None;
    }
    XmlFragment::parse(body).ok()?;
    let opening_end = body.find('>')?;
    let opening = &body[..=opening_end];
    let tag_end = opening[1..]
        .find(|character: char| character.is_ascii_whitespace() || character == '>')?
        + 1;
    let tag = &opening[1..tag_end];
    if !valid_xml_name(tag) {
        return None;
    }
    let closing_length = tag.len() + 3;
    if body.len() < opening.len() + closing_length {
        return None;
    }
    let closing_start = body.len() - closing_length;
    let closing = &body[closing_start..];
    if closing.strip_prefix("</")?.strip_suffix('>')? != tag {
        return None;
    }
    Some(InlineElement {
        opening,
        content: &body[opening_end + 1..closing_start],
        closing,
    })
}

fn documentation_body(comment: &str) -> &str {
    comment
        .strip_prefix("--- ")
        .or_else(|| comment.strip_prefix("---"))
        .unwrap_or(comment)
        .trim_end()
}

fn valid_closing_tag(body: &str) -> bool {
    body.strip_prefix("</")
        .and_then(|name| name.strip_suffix('>'))
        .is_some_and(valid_xml_name)
}

fn valid_xml_name(name: &str) -> bool {
    let mut characters = name.chars();
    characters
        .next()
        .is_some_and(|character| character.is_ascii_alphabetic() || character == '_')
        && characters.all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | ':' | '.')
        })
}

fn line_indentation(text: &str, offset: usize) -> &str {
    let line_start = text[..offset]
        .rfind('\n')
        .map_or(0, |position| position + 1);
    &text[line_start..offset]
}

fn is_adjacent_gap(gap: &str, indentation: &str) -> bool {
    gap.strip_suffix(indentation)
        .is_some_and(|newline| matches!(newline, "\n" | "\r\n"))
}
