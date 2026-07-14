//! Lossless Pop Lang tokenization and syntax parsing.
//!
//! Ownership follows the grammar rather than file size: `lexer` preserves every
//! token; `body` owns blocks and statements; `expression` owns precedence and
//! postfix forms; declaration modules own their corresponding surface syntax.
//! The root parser only discovers file-scoped declarations. Unimplemented grammar
//! is never assigned semantic meaning here.

use pop_diagnostics::syntax as syntax_diagnostics;
use pop_foundation::{Diagnostic, SourceSpan, TextRange, TextSize};
use pop_source::SourceFile;

mod attribute;
mod body;
mod class;
mod constant;
mod data;
mod expression;
mod interface;
mod lexer;
mod signature;
mod string_literal;
mod type_alias;

pub use attribute::{
    AttributeArgumentSyntax, AttributeDeclarationSyntax, AttributeParameterSyntax,
    AttributeSyntaxError, AttributeUseSyntax, parse_attribute_declaration, parse_attribute_use,
};
pub use body::{
    BinaryOperator, CaptureFunctionParameterSyntax, CaptureFunctionSyntax, ExpressionSyntax,
    ExpressionSyntaxKind, FieldInitializerSyntax, FunctionBodyError, FunctionBodySyntax,
    LocalBindingSyntax, MatchArmSyntax, StatementSyntax, StatementSyntaxKind, StringSegmentSyntax,
    StringSegmentSyntaxKind, UnaryOperator, parse_function_body,
};
pub use class::{
    ClassDeclarationError, ClassDeclarationSyntax, ClassFieldSyntax, ClassMethodDispatchSyntax,
    ClassMethodParameterSyntax, ClassMethodSyntax, VisibilitySyntax, parse_class_declaration,
    parse_class_method_body,
};
pub use constant::{ConstDeclarationError, ConstDeclarationSyntax, parse_const_declaration};
pub use data::{
    DataDeclarationError, EnumCaseSyntax, EnumDeclarationSyntax, ErrorCaseParameterSyntax,
    ErrorCaseSyntax, ErrorDeclarationSyntax, RecordDeclarationSyntax, RecordFieldSyntax,
    UnionCaseParameterSyntax, UnionCaseSyntax, UnionDeclarationSyntax, parse_enum_declaration,
    parse_error_declaration, parse_record_declaration, parse_union_declaration,
};
pub use interface::{
    InterfaceDeclarationError, InterfaceDeclarationSyntax, InterfaceMethodParameterSyntax,
    InterfaceMethodSyntax, parse_interface_declaration,
};
pub use lexer::{LexResult, Token, TokenKind, lex};
pub use signature::{
    FunctionParameterSyntax, FunctionSignatureError, FunctionSignatureSyntax,
    GenericParameterSyntax, TypeSyntax, TypeSyntaxKind, parse_function_signature,
};
pub use string_literal::{StringLiteralError, decode_string_literal};
pub use type_alias::{
    TypeAliasDeclarationError, TypeAliasDeclarationSyntax, parse_type_alias_declaration,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NodeKind {
    Root,
    NamespaceDeclaration,
    UsingDirective,
    AttributeUse,
    FunctionDeclaration,
    ConstDeclaration,
    TypeAliasDeclaration,
    AttributeDeclaration,
    RecordDeclaration,
    UnionDeclaration,
    ErrorDeclaration,
    ClassDeclaration,
    InterfaceDeclaration,
    EnumDeclaration,
    Error,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SyntaxNode {
    kind: NodeKind,
    range: TextRange,
    children: Vec<SyntaxNode>,
}

impl SyntaxNode {
    #[must_use]
    pub const fn kind(&self) -> NodeKind {
        self.kind
    }

    #[must_use]
    pub const fn range(&self) -> TextRange {
        self.range
    }

    #[must_use]
    pub fn children(&self) -> &[Self] {
        &self.children
    }
}

#[derive(Clone, Debug)]
pub struct SyntaxTree {
    root: SyntaxNode,
    tokens: Vec<Token>,
    diagnostics: Vec<Diagnostic>,
}

impl SyntaxTree {
    #[must_use]
    pub const fn root(&self) -> &SyntaxNode {
        &self.root
    }

    #[must_use]
    pub fn tokens(&self) -> &[Token] {
        &self.tokens
    }

    #[must_use]
    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    #[must_use]
    pub fn reconstruct(&self, source: &SourceFile) -> String {
        reconstruct_tokens(&self.tokens, source)
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

#[must_use]
pub fn parse_file(source: &SourceFile) -> SyntaxTree {
    let lexed = lex(source);
    let mut diagnostics = lexed.diagnostics().to_vec();
    let tokens = lexed.tokens().to_vec();
    let mut parser = Parser {
        source,
        tokens: &tokens,
        diagnostics: &mut diagnostics,
    };
    let children = parser.parse_root();
    diagnostics.sort_by_key(|diagnostic| {
        let range = diagnostic.primary_span().range();
        (
            range.start().to_u32(),
            range.end().to_u32(),
            diagnostic.code().as_str(),
        )
    });
    let root_range = TextRange::new(TextSize::from_u32(0), source.len())
        .unwrap_or_else(|| TextRange::empty(TextSize::from_u32(0)));
    let root = SyntaxNode {
        kind: NodeKind::Root,
        range: root_range,
        children,
    };
    SyntaxTree {
        root,
        tokens,
        diagnostics,
    }
}

fn reconstruct_tokens(tokens: &[Token], source: &SourceFile) -> String {
    let mut text = String::with_capacity(source.text().len());
    for token in tokens {
        text.push_str(token.text(source));
    }
    text
}

struct Parser<'source, 'diagnostics> {
    source: &'source SourceFile,
    tokens: &'source [Token],
    diagnostics: &'diagnostics mut Vec<Diagnostic>,
}

impl Parser<'_, '_> {
    fn parse_root(&mut self) -> Vec<SyntaxNode> {
        let mut children = Vec::new();
        let mut cursor = 0;
        let first = self.next_significant(cursor);
        if let Some(namespace) =
            first.filter(|index| self.tokens[*index].kind() == TokenKind::Namespace)
        {
            let end = self.line_end(namespace);
            children.push(self.node_for_tokens(NodeKind::NamespaceDeclaration, namespace, end));
            cursor = end;
        } else {
            let start = TextSize::from_u32(0);
            self.diagnostics
                .push(syntax_diagnostics::missing_namespace(SourceSpan::new(
                    self.source.id(),
                    TextRange::empty(start),
                )));
        }

        loop {
            let Some(index) = self.next_significant(cursor) else {
                return children;
            };
            if self.tokens[index].kind() != TokenKind::Using {
                break;
            }
            let end = self.line_end(index);
            children.push(self.node_for_tokens(NodeKind::UsingDirective, index, end));
            cursor = end;
        }

        while let Some(index) = self.next_significant(cursor) {
            let kind = self.tokens[index].kind();
            if kind == TokenKind::At {
                let end = self.line_end(index);
                children.push(self.node_for_tokens(NodeKind::AttributeUse, index, end));
                cursor = end;
                continue;
            }

            let (start, mut declaration) = match kind {
                TokenKind::Public | TokenKind::Internal | TokenKind::Private => {
                    let Some(declaration) = self.next_significant(index + 1) else {
                        children.push(self.error_line(index));
                        break;
                    };
                    (index, declaration)
                }
                TokenKind::Export => {
                    self.diagnostics
                        .push(syntax_diagnostics::unsupported_export(self.span(index)));
                    let Some(declaration) = self.next_significant(index + 1) else {
                        children.push(self.error_line(index));
                        break;
                    };
                    (index, declaration)
                }
                kind if self.declaration_node_kind(index).is_some()
                    || matches!(kind, TokenKind::Open | TokenKind::Async) =>
                {
                    (index, index)
                }
                _ => {
                    children.push(self.error_line(index));
                    cursor = self.line_end(index);
                    continue;
                }
            };

            if self.tokens[declaration].kind() == TokenKind::Open {
                let Some(class) = self.next_significant(declaration + 1) else {
                    children.push(self.error_line(start));
                    break;
                };
                declaration = class;
            }

            if self.tokens[declaration].kind() == TokenKind::Async {
                let Some(function) = self.next_significant(declaration + 1) else {
                    children.push(self.error_line(start));
                    break;
                };
                declaration = function;
            }

            let Some((node_kind, is_block)) = self.declaration_node_kind(declaration) else {
                let found = self.tokens[declaration].text(self.source).to_owned();
                self.diagnostics.push(syntax_diagnostics::unexpected_token(
                    self.span(declaration),
                    "namespace-scope declaration",
                    found,
                ));
                children.push(self.error_line(start));
                cursor = self.line_end(start);
                continue;
            };

            let end = if is_block {
                self.block_end(declaration)
            } else {
                self.line_end(declaration)
            };
            children.push(self.node_for_tokens(node_kind, start, end));
            cursor = end;
        }

        children
    }

    fn block_end(&mut self, declaration: usize) -> usize {
        let interface_declaration = self.tokens[declaration].kind() == TokenKind::Interface;
        let mut depth = 1_u32;
        let mut cursor = declaration + 1;
        while let Some(index) = self.next_significant(cursor) {
            match self.tokens[index].kind() {
                TokenKind::Function
                    if !interface_declaration && self.function_opens_block(index) =>
                {
                    depth += 1;
                }
                TokenKind::If if self.if_opens_block(index) => depth += 1,
                TokenKind::While
                | TokenKind::For
                | TokenKind::Record
                | TokenKind::Class
                | TokenKind::Interface
                | TokenKind::Union
                | TokenKind::Enum
                | TokenKind::Match
                | TokenKind::Defer => depth += 1,
                TokenKind::End => {
                    depth -= 1;
                    if depth == 0 {
                        return index + 1;
                    }
                }
                _ => {}
            }
            cursor = index + 1;
        }

        let end = self.source.len();
        self.diagnostics.push(syntax_diagnostics::unexpected_token(
            SourceSpan::new(self.source.id(), TextRange::empty(end)),
            "`end`",
            "end of file",
        ));
        self.tokens.len()
    }

    fn error_line(&mut self, start: usize) -> SyntaxNode {
        let found = self.tokens[start].text(self.source).to_owned();
        self.diagnostics.push(syntax_diagnostics::unexpected_token(
            self.span(start),
            "namespace-scope declaration",
            found,
        ));
        let end = self.line_end(start);
        self.node_for_tokens(NodeKind::Error, start, end)
    }

    fn next_significant(&self, mut cursor: usize) -> Option<usize> {
        while let Some(token) = self.tokens.get(cursor) {
            if !token.kind().is_trivia() {
                return Some(cursor);
            }
            cursor += 1;
        }
        None
    }

    fn function_opens_block(&self, index: usize) -> bool {
        let mut significant = self.tokens[..index]
            .iter()
            .rev()
            .filter(|token| !token.kind().is_trivia());
        let Some(previous) = significant.next() else {
            return true;
        };
        if previous.kind() == TokenKind::Colon {
            return false;
        }
        if previous.kind() == TokenKind::Async {
            return significant
                .next()
                .is_none_or(|token| token.kind() != TokenKind::Colon);
        }
        true
    }

    fn if_opens_block(&self, index: usize) -> bool {
        for token in self.tokens[..index].iter().rev() {
            match token.kind() {
                TokenKind::Newline => return true,
                TokenKind::Whitespace
                | TokenKind::LineComment
                | TokenKind::DocumentationComment => {}
                _ => return false,
            }
        }
        true
    }

    fn line_end(&self, start: usize) -> usize {
        let mut cursor = start;
        while let Some(token) = self.tokens.get(cursor) {
            cursor += 1;
            if token.kind() == TokenKind::Newline {
                break;
            }
        }
        cursor
    }

    fn node_for_tokens(&self, kind: NodeKind, start: usize, end: usize) -> SyntaxNode {
        let start_offset = self.tokens[start].range().start();
        let end_offset = end
            .checked_sub(1)
            .and_then(|index| self.tokens.get(index))
            .map_or(start_offset, |token| token.range().end());
        let range = TextRange::new(start_offset, end_offset)
            .unwrap_or_else(|| TextRange::empty(start_offset));
        SyntaxNode {
            kind,
            range,
            children: Vec::new(),
        }
    }

    fn span(&self, token: usize) -> SourceSpan {
        SourceSpan::new(self.source.id(), self.tokens[token].range())
    }

    fn declaration_node_kind(&self, index: usize) -> Option<(NodeKind, bool)> {
        let token = self.tokens.get(index)?;
        if token.kind() == TokenKind::Identifier && token.text(self.source) == "error" {
            return Some((NodeKind::ErrorDeclaration, true));
        }
        declaration_node_kind(token.kind())
    }
}

const fn declaration_node_kind(kind: TokenKind) -> Option<(NodeKind, bool)> {
    Some(match kind {
        TokenKind::Function => (NodeKind::FunctionDeclaration, true),
        TokenKind::Const => (NodeKind::ConstDeclaration, false),
        TokenKind::Type => (NodeKind::TypeAliasDeclaration, false),
        TokenKind::Attribute => (NodeKind::AttributeDeclaration, false),
        TokenKind::Record => (NodeKind::RecordDeclaration, true),
        TokenKind::Union => (NodeKind::UnionDeclaration, true),
        TokenKind::Class => (NodeKind::ClassDeclaration, true),
        TokenKind::Interface => (NodeKind::InterfaceDeclaration, true),
        TokenKind::Enum => (NodeKind::EnumDeclaration, true),
        _ => return None,
    })
}
