//! Deterministic `documentation.xml` rendering from checked XML fragments.

use std::collections::BTreeSet;
use std::error::Error;
use std::fmt;
use std::fmt::Write as _;

use pop_documentation::{XmlFragment, XmlNode};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DocumentationMember {
    id: String,
    documentation: XmlFragment,
}

impl DocumentationMember {
    #[must_use]
    pub fn new(id: impl Into<String>, documentation: XmlFragment) -> Self {
        Self {
            id: id.into(),
            documentation,
        }
    }

    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    #[must_use]
    pub const fn documentation(&self) -> &XmlFragment {
        &self.documentation
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DocumentationOutputError {
    DuplicateMemberId(String),
}

impl fmt::Display for DocumentationOutputError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateMemberId(id) => {
                write!(formatter, "duplicate documentation member ID `{id}`")
            }
        }
    }
}

impl Error for DocumentationOutputError {}

/// Renders the schema-versioned public documentation companion artifact.
///
/// # Errors
///
/// Rejects duplicate stable member IDs rather than silently replacing docs.
pub fn render_xml(
    bubble: &str,
    members: &[DocumentationMember],
) -> Result<String, DocumentationOutputError> {
    let mut members = members.to_vec();
    members.sort_by(|left, right| left.id.cmp(&right.id));
    let mut seen = BTreeSet::new();
    for member in &members {
        if !seen.insert(member.id.clone()) {
            return Err(DocumentationOutputError::DuplicateMemberId(
                member.id.clone(),
            ));
        }
    }

    let mut output = String::new();
    output.push_str("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n");
    output.push_str("<doc schemaVersion=\"1\" bubble=\"");
    write_escaped_attribute(&mut output, bubble);
    output.push_str("\">\n  <members>\n");
    for member in members {
        output.push_str("    <member id=\"");
        write_escaped_attribute(&mut output, &member.id);
        output.push_str("\">");
        render_nodes(&mut output, member.documentation.children());
        output.push_str("</member>\n");
    }
    output.push_str("  </members>\n</doc>\n");
    Ok(output)
}

fn render_nodes(output: &mut String, nodes: &[XmlNode]) {
    for node in nodes {
        match node {
            XmlNode::Text(text) => output.push_str(text),
            XmlNode::Element {
                name,
                attributes,
                children,
            } => {
                output.push('<');
                output.push_str(name);
                let mut attributes = attributes.iter().collect::<Vec<_>>();
                attributes.sort_by(|left, right| {
                    (left.name(), left.value()).cmp(&(right.name(), right.value()))
                });
                for attribute in attributes {
                    output.push(' ');
                    output.push_str(attribute.name());
                    output.push_str("=\"");
                    output.push_str(attribute.value());
                    output.push('"');
                }
                if children.is_empty() {
                    output.push_str("/>");
                } else {
                    output.push('>');
                    render_nodes(output, children);
                    let _ = write!(output, "</{name}>");
                }
            }
        }
    }
}

fn write_escaped_attribute(output: &mut String, value: &str) {
    for character in value.chars() {
        match character {
            '&' => output.push_str("&amp;"),
            '<' => output.push_str("&lt;"),
            '>' => output.push_str("&gt;"),
            '"' => output.push_str("&quot;"),
            '\'' => output.push_str("&apos;"),
            _ => output.push(character),
        }
    }
}
