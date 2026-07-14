use pop_diagnostics::resolution as diagnostics;
use pop_foundation::{Diagnostic, ModuleId, SourceSpan, SymbolId};

use crate::model::{Declaration, DeclarationIndex, SymbolSpace};

#[derive(Clone, Debug)]
pub struct Resolution {
    symbols: Vec<SymbolId>,
    diagnostics: Vec<Diagnostic>,
}

impl Resolution {
    #[must_use]
    pub fn symbol(&self) -> Option<SymbolId> {
        (self.symbols.len() == 1).then(|| self.symbols[0])
    }

    #[must_use]
    pub fn symbols(&self) -> &[SymbolId] {
        &self.symbols
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

#[derive(Clone, Debug)]
pub struct ResolutionDatabase {
    index: DeclarationIndex,
}

impl ResolutionDatabase {
    #[must_use]
    pub const fn new(index: DeclarationIndex) -> Self {
        Self { index }
    }

    #[must_use]
    pub const fn index(&self) -> &DeclarationIndex {
        &self.index
    }

    #[must_use]
    pub fn resolve(
        &self,
        module: ModuleId,
        name: &str,
        space: SymbolSpace,
        use_span: SourceSpan,
    ) -> Resolution {
        let Some(context) = self.index.module(module) else {
            return unknown(name, use_span);
        };
        if name.contains('.') {
            let (namespace, simple_name) = expand_qualified_name(context, name);
            let group = self.index.lookup(&namespace, &simple_name, space);
            return evaluate_groups(vec![group], module, context.bubble(), name, use_span);
        }

        let current = self.index.lookup(context.namespace(), name, space);
        if !current.is_empty() {
            return evaluate_groups(vec![current], module, context.bubble(), name, use_span);
        }

        let groups: Vec<_> = context
            .usings()
            .iter()
            .filter(|using| using.alias().is_none())
            .map(|using| self.index.lookup(using.namespace(), name, space))
            .filter(|group| !group.is_empty())
            .collect();
        evaluate_groups(groups, module, context.bubble(), name, use_span)
    }
}

fn expand_qualified_name(context: &crate::model::ModuleIndex, name: &str) -> (String, String) {
    let mut components: Vec<_> = name.split('.').collect();
    let simple_name = components.pop().unwrap_or_default().to_owned();
    if let Some(first) = components.first_mut()
        && let Some(using) = context
            .usings()
            .iter()
            .find(|using| using.alias() == Some(*first))
    {
        let mut namespace: Vec<_> = using.namespace().split('.').collect();
        namespace.extend(components.iter().skip(1).copied());
        return (namespace.join("."), simple_name);
    }
    (components.join("."), simple_name)
}

fn evaluate_groups(
    groups: Vec<Vec<&Declaration>>,
    module: ModuleId,
    bubble: pop_foundation::BubbleId,
    name: &str,
    use_span: SourceSpan,
) -> Resolution {
    let existing: Vec<_> = groups
        .into_iter()
        .filter(|group| !group.is_empty())
        .collect();
    if existing.is_empty() {
        return unknown(name, use_span);
    }
    let accessible: Vec<Vec<_>> = existing
        .iter()
        .map(|group| {
            group
                .iter()
                .copied()
                .filter(|declaration| declaration.is_accessible_from(module, bubble))
                .collect()
        })
        .filter(|group: &Vec<_>| !group.is_empty())
        .collect();
    if accessible.len() > 1 {
        let diagnostic = diagnostics::ambiguous_name(
            use_span,
            name,
            accessible
                .iter()
                .filter_map(|group| group.first().map(|declaration| declaration.span())),
        );
        return Resolution {
            symbols: Vec::new(),
            diagnostics: vec![diagnostic],
        };
    }
    if let Some(group) = accessible.first() {
        return Resolution {
            symbols: group
                .iter()
                .map(|declaration| declaration.symbol())
                .collect(),
            diagnostics: Vec::new(),
        };
    }
    let declaration = existing[0][0];
    Resolution {
        symbols: Vec::new(),
        diagnostics: vec![diagnostics::inaccessible_name(
            use_span,
            name,
            declaration.span(),
        )],
    }
}

fn unknown(name: &str, span: SourceSpan) -> Resolution {
    Resolution {
        symbols: Vec::new(),
        diagnostics: vec![diagnostics::unknown_name(span, name)],
    }
}
