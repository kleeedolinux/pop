use std::collections::BTreeMap;

use pop_diagnostics::resolution as diagnostics;
use pop_foundation::{BubbleId, Diagnostic, ModuleId, SourceSpan, SymbolId, TextRange};
use pop_source::SourceFile;
use pop_syntax::{NodeKind, SyntaxNode, SyntaxTree, Token, TokenKind};

use crate::model::{
    Declaration, DeclarationIndex, DeclarationKind, DeclarationOwner, ModuleIndex, SymbolSpace,
    UsingDirective, Visibility,
};

#[derive(Clone, Copy)]
pub struct ModuleInput<'source> {
    module: ModuleId,
    bubble: BubbleId,
    source: &'source SourceFile,
    syntax: &'source SyntaxTree,
}

impl<'source> ModuleInput<'source> {
    #[must_use]
    pub const fn new(
        module: ModuleId,
        bubble: BubbleId,
        source: &'source SourceFile,
        syntax: &'source SyntaxTree,
    ) -> Self {
        Self {
            module,
            bubble,
            source,
            syntax,
        }
    }
}

#[derive(Clone, Debug)]
pub struct IndexResult {
    index: DeclarationIndex,
    diagnostics: Vec<Diagnostic>,
}

impl IndexResult {
    #[must_use]
    pub const fn index(&self) -> &DeclarationIndex {
        &self.index
    }

    #[must_use]
    pub fn into_index(self) -> DeclarationIndex {
        self.index
    }

    #[must_use]
    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }
}

#[must_use]
pub fn build_declaration_index(inputs: &[ModuleInput<'_>]) -> IndexResult {
    let mut ordered: Vec<_> = inputs.iter().collect();
    ordered.sort_by_key(|input| input.module);
    let mut modules = BTreeMap::new();
    let mut builder = IndexBuilder::new();

    for input in ordered {
        if modules.contains_key(&input.module) {
            builder
                .diagnostics
                .push(diagnostics::invalid_declaration(root_span(input.source)));
            continue;
        }
        modules.insert(input.module, builder.index_module(input));
    }
    builder.finish(modules)
}

struct IndexBuilder {
    declarations: BTreeMap<SymbolId, Declaration>,
    diagnostics: Vec<Diagnostic>,
    next_symbol: u32,
    declared_names: BTreeMap<(String, SymbolSpace, String), SourceSpan>,
}

impl IndexBuilder {
    const fn new() -> Self {
        Self {
            declarations: BTreeMap::new(),
            diagnostics: Vec::new(),
            next_symbol: 0,
            declared_names: BTreeMap::new(),
        }
    }

    fn index_module(&mut self, input: &ModuleInput<'_>) -> ModuleIndex {
        self.diagnostics
            .extend(input.syntax.diagnostics().iter().cloned());
        let namespace = input
            .syntax
            .root()
            .children()
            .iter()
            .find(|node| node.kind() == NodeKind::NamespaceDeclaration)
            .and_then(|node| qualified_name_after(input, node, TokenKind::Namespace))
            .unwrap_or_default();
        if namespace.is_empty() {
            self.diagnostics
                .push(diagnostics::invalid_declaration(root_span(input.source)));
        }
        let usings: Vec<_> = input
            .syntax
            .root()
            .children()
            .iter()
            .filter(|node| node.kind() == NodeKind::UsingDirective)
            .filter_map(|node| parse_using(input, node))
            .collect();
        let module_symbols = input
            .syntax
            .root()
            .children()
            .iter()
            .filter_map(|node| self.index_declaration(input, node, &namespace))
            .collect();
        ModuleIndex::new(
            input.module,
            input.bubble,
            namespace,
            usings,
            module_symbols,
        )
    }

    fn index_declaration(
        &mut self,
        input: &ModuleInput<'_>,
        node: &SyntaxNode,
        namespace: &str,
    ) -> Option<SymbolId> {
        let kind = declaration_kind(node.kind())?;
        let tokens = significant_tokens(input, node);
        let visibility = tokens.iter().find_map(|token| visibility(token.kind()))?;
        let Some(keyword_index) = tokens
            .iter()
            .position(|token| token_kind_matches_declaration(token.kind(), kind))
        else {
            self.diagnostics
                .push(diagnostics::invalid_declaration(node_span(input, node)));
            return None;
        };
        let Some(name_token) = tokens[keyword_index + 1..]
            .iter()
            .find(|token| token.kind() == TokenKind::Identifier)
        else {
            self.diagnostics
                .push(diagnostics::invalid_declaration(node_span(input, node)));
            return None;
        };
        let name = name_token.text(input.source).to_owned();
        let span = SourceSpan::new(input.source.id(), name_token.range());
        let key = (namespace.to_owned(), kind.symbol_space(), name.clone());
        if kind != DeclarationKind::Function {
            if let Some(original) = self.declared_names.get(&key) {
                self.diagnostics
                    .push(diagnostics::duplicate_declaration(span, name, *original));
                return None;
            }
            self.declared_names.insert(key, span);
        }
        let symbol = SymbolId::from_raw(self.next_symbol);
        let Some(incremented) = self.next_symbol.checked_add(1) else {
            self.diagnostics
                .push(diagnostics::invalid_declaration(span));
            return None;
        };
        self.next_symbol = incremented;
        self.declarations.insert(
            symbol,
            Declaration::new_in_namespace(
                symbol,
                DeclarationOwner::new(input.module, input.bubble, namespace),
                name,
                kind,
                visibility,
                span,
            ),
        );
        Some(symbol)
    }

    fn finish(mut self, modules: BTreeMap<ModuleId, ModuleIndex>) -> IndexResult {
        self.diagnostics.sort_by_key(|diagnostic| {
            let span = diagnostic.primary_span();
            (
                span.file(),
                span.range().start(),
                diagnostic.code().as_str(),
            )
        });
        IndexResult {
            index: DeclarationIndex::new(modules, self.declarations),
            diagnostics: self.diagnostics,
        }
    }
}

fn parse_using(input: &ModuleInput<'_>, node: &SyntaxNode) -> Option<UsingDirective> {
    let tokens = significant_tokens(input, node);
    let names: Vec<_> = tokens
        .iter()
        .enumerate()
        .filter(|(_, token)| token.kind() == TokenKind::Identifier)
        .collect();
    let equal = tokens
        .iter()
        .position(|token| token.kind() == TokenKind::Equal);
    let (alias, namespace_start) = if let Some(equal) = equal {
        let alias = names
            .iter()
            .find(|(index, _)| *index < equal)
            .map(|(_, token)| token.text(input.source).to_owned())?;
        (Some(alias), equal + 1)
    } else {
        (None, 1)
    };
    let namespace = qualified_name_from_tokens(input.source, &tokens[namespace_start..]);
    if namespace.is_empty() {
        return None;
    }
    Some(UsingDirective::new(
        namespace,
        alias,
        node_span(input, node),
    ))
}

fn qualified_name_after(
    input: &ModuleInput<'_>,
    node: &SyntaxNode,
    keyword: TokenKind,
) -> Option<String> {
    let tokens = significant_tokens(input, node);
    let keyword = tokens.iter().position(|token| token.kind() == keyword)?;
    let name = qualified_name_from_tokens(input.source, &tokens[keyword + 1..]);
    (!name.is_empty()).then_some(name)
}

fn qualified_name_from_tokens(source: &SourceFile, tokens: &[&Token]) -> String {
    tokens
        .iter()
        .take_while(|token| matches!(token.kind(), TokenKind::Identifier | TokenKind::Dot))
        .map(|token| token.text(source))
        .collect()
}

fn significant_tokens<'a>(input: &'a ModuleInput<'_>, node: &SyntaxNode) -> Vec<&'a Token> {
    input
        .syntax
        .tokens()
        .iter()
        .filter(|token| {
            !token.kind().is_trivia()
                && token.range().start() >= node.range().start()
                && token.range().end() <= node.range().end()
        })
        .collect()
}

const fn declaration_kind(kind: NodeKind) -> Option<DeclarationKind> {
    Some(match kind {
        NodeKind::FunctionDeclaration => DeclarationKind::Function,
        NodeKind::ConstDeclaration => DeclarationKind::Constant,
        NodeKind::TypeAliasDeclaration => DeclarationKind::TypeAlias,
        NodeKind::AttributeDeclaration => DeclarationKind::Attribute,
        NodeKind::RecordDeclaration => DeclarationKind::Record,
        NodeKind::UnionDeclaration => DeclarationKind::Union,
        NodeKind::ClassDeclaration => DeclarationKind::Class,
        NodeKind::InterfaceDeclaration => DeclarationKind::Interface,
        NodeKind::EnumDeclaration => DeclarationKind::Enum,
        _ => return None,
    })
}

const fn token_kind_matches_declaration(token: TokenKind, kind: DeclarationKind) -> bool {
    matches!(
        (token, kind),
        (TokenKind::Function, DeclarationKind::Function)
            | (TokenKind::Const, DeclarationKind::Constant)
            | (TokenKind::Type, DeclarationKind::TypeAlias)
            | (TokenKind::Attribute, DeclarationKind::Attribute)
            | (TokenKind::Record, DeclarationKind::Record)
            | (TokenKind::Union, DeclarationKind::Union)
            | (TokenKind::Class, DeclarationKind::Class)
            | (TokenKind::Interface, DeclarationKind::Interface)
            | (TokenKind::Enum, DeclarationKind::Enum)
    )
}

const fn visibility(kind: TokenKind) -> Option<Visibility> {
    match kind {
        TokenKind::Public => Some(Visibility::Public),
        TokenKind::Internal => Some(Visibility::Internal),
        TokenKind::Private => Some(Visibility::Private),
        _ => None,
    }
}

fn node_span(input: &ModuleInput<'_>, node: &SyntaxNode) -> SourceSpan {
    SourceSpan::new(input.source.id(), node.range())
}

fn root_span(source: &SourceFile) -> SourceSpan {
    SourceSpan::new(
        source.id(),
        TextRange::new(pop_foundation::TextSize::from_u32(0), source.len())
            .unwrap_or_else(|| TextRange::empty(pop_foundation::TextSize::from_u32(0))),
    )
}
