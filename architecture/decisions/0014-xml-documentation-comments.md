# ADR 0014: XML Documentation Comments

- Status: accepted
- Date: 2026-07-10

## Context

Pop Lang needs structured API documentation that editors, compilers, libraries,
and documentation generators can share. C# XML documentation demonstrates a
useful model, but `///`, exception-centered tags, broad XML inclusion, and
reflection-oriented consumption do not fit Pop Lang unchanged.

## Decision

Pop Lang accepts `---` XML documentation comments immediately before
declarations. The compiler parses safe XML, attaches it to resolved symbols,
validates parameter/type-parameter/return/error/effect/reference contracts,
emits structured diagnostics/quick fixes, and writes `documentation.xml` beside
reference metadata in distributable `.poplib` artifacts.

Core C# concepts such as `<summary>`, `<remarks>`, `<param>`, `<typeparam>`,
`<returns>`, `<example>`, `<see>`, and `<inheritdoc>` are supported. Pop adds
typed `<error>`, `<panic>`, `<effect>`, `<complexity>`, `<allocation>`, and
`<threadSafety>` tags.

DTD/entity/include/XPath/source-evaluation capabilities are excluded initially.
`cref` resolves statically and emits stable documentation IDs; it is not runtime
reflection. Documentation changes use a separate hash and do not alter ABI/
runtime artifacts.

## Consequences

- The lexer/parser/lossless tree need documentation-token attachment.
- The compiler needs a safe XML parser, semantic doc validator, incremental doc
  queries, catalog diagnostics, and quick-fix providers.
- `.poplib` manifests gain a separate documentation hash/artifact.
- LSP hover/completion/signature help and `pop documentation` share one semantic model.
- Standard-library public APIs require complete checked documentation.
- Documentation examples can be compiled in CI without becoming string mixins.

## Alternatives considered

### Copy C# `///` exactly

Rejected because `---` is the natural extension of Lua/Luau `--` comments and
keeps Pop Lang visually coherent.

### Use Markdown-only comments

Rejected as the only representation because parameter/type/error/effect/link
contracts need reliable structured fields. Markdown can still appear inside
safe text/custom renderer conventions where specified.

### Treat documentation as unparsed trivia

Rejected because stale names, parameters, errors, and examples would silently
degrade editor/library quality.

### Retain documentation as runtime reflection

Rejected because editor/doc artifacts can consume it without increasing runtime
metadata or enabling dynamic member access.
