# ADR 0089: Private Semantic Language-Server Queries

- Status: accepted
- Date: 2026-07-16
- Supersedes: none
- Amends: ADR 0014, ADR 0033, ADR 0088

## Context

The private LSP 3.17 adapter currently publishes diagnostics from the lossless
syntax parser only. The compiler already owns structured semantic diagnostics
and checked XML documentation, while ADR 0014 requires editor hover to consume
that shared documentation model. Implementing hover or semantic diagnostics in
the editor adapter through a second parser, textual matching, or CLI output
scraping would create competing language semantics.

The public `Pop.Syntax` and `Pop.Lsp` Packages remain bootstrap manifests. Their
future source, protocol, and query schemas are not yet stable enough to expose
compiler-private trees, arenas, or query handles.

## Decision

The private language server may consume a version-coupled compiler tooling
projection for three bounded LSP features:

- compiler diagnostics for the current immutable document snapshot;
- hover for a namespace-scope declaration, containing its exact source
  signature and checked XML documentation summary; and
- hierarchical document symbols derived from the compiler declaration index.

The compiler front end remains the only semantic authority. A tooling
projection contains stable typed identities, declaration kinds, source spans,
and checked documentation. It never exposes resolver databases, syntax arenas,
HIR/MIR nodes, runtime values, or string-based symbol lookup. Hover selects a
declaration by its indexed source span and identity. Invalid or incomplete
source may publish diagnostics without publishing semantic hover data.

The bootstrap server analyzes one open Module as one session-owned Bubble until
Package/Workspace snapshot queries are implemented. Session `BubbleId`,
`ModuleId`, and `NamespaceId` values are typed, deterministic, and private to
the query; they are not serialized identities and do not claim Package
resolution. The server never guesses identity from filenames or namespaces.

The private LSP wire schema follows LSP 3.17 for
`textDocument/hover` and `textDocument/documentSymbol`. Requests use the
currently published immutable snapshot, are cancellation points, use UTF-16
positions, and return `null` when no declaration is selected. Documentation is
rendered as safe Markdown plain text; raw XML and executable markup are not
sent to clients.

Completion, signature help, cross-Module definition/references, rename,
formatting, semantic tokens, code actions, incremental text edits, and
Workspace analysis remain outside this decision. They require reviewed query
and edit contracts rather than textual approximations.

## Consequences

- The language server depends on the compiler driver instead of treating the
  syntax parser as a semantic compiler.
- Semantic and documentation diagnostics appear in editors with the same
  stable codes and localized presentation used by the CLI.
- Hover and document symbols work for accepted namespace-scope declarations;
  richer body/local/member queries remain planned.
- The private Rust tooling projection can change with the toolchain and does
  not stabilize the independently installable `Pop.Lsp` Package.

## Alternatives considered

### Shell out to `pop check`

Rejected because tools must consume compiler/query APIs and structured facts,
not parse human CLI output or pay a process launch for each editor snapshot.

### Derive semantic features from syntax tokens

Rejected because token text cannot prove symbol identity, visibility, types,
or checked documentation and would diverge from the compiler.

### Stabilize every planned LSP feature now

Rejected because completion, rename, navigation, edits, and Workspace queries
still have unresolved indexing and public-schema dependencies.

### Re-export private compiler values through `Pop.Lsp`

Rejected by ADR 0033. Public tooling Packages require reviewed stable facades.

## Required conformance tests

- a syntactically valid semantic error is published with its compiler code;
- checked documentation and the exact declaration signature appear in hover;
- malformed documentation produces diagnostics and no unchecked hover content;
- hover outside a declaration returns `null`;
- document symbols preserve declaration kind, name, and UTF-16 range;
- stale or closed snapshots cannot be queried;
- advertised capabilities exactly match implemented requests; and
- architecture checks prevent CLI scraping and compiler-private value export.

## Documents/components affected

CLI/tooling architecture, implementation roadmap, architecture conformance,
compiler driver tooling projections, private language server, LSP transport,
official editor extensions, and language-server tests.
