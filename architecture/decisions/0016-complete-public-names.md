# ADR 0016: Complete Public Names

- Status: accepted
- Date: 2026-07-10

## Context

Pop Lang should remain visually compact without making APIs cryptic. Namespace
context already removes repeated qualifiers, so arbitrary truncations such as
`Iter` do not buy enough brevity to justify ambiguity. In particular,
`iter.map` does not say whether `iter` is an iterable value, iterator state, or
an algorithm namespace.

## Decision

Public language and library identifiers use complete words. The sequence
protocols are `Iterable<T>` and `Iterator<T>`; algorithms live under
`Sequence.map`, `Sequence.filter`, and `Sequence.fold`. `Iter` and `iter.map`
are not accepted standard API names.

Arbitrary truncations such as `Config`, `Sync`, `Mgr`, and `Util` are forbidden
in canonical/public APIs. Established technical initialisms and conventional
domain words remain permitted and are cased as words: `Json`, `Http`, `Io`,
`Utf8`, `Ffi`, `Gc`, `Guid`, and `Async`.

Namespaces provide the brevity: prefer `Json.Value`, `Http.Client`, and
`Text.Builder` rather than repeating context in long type names. Types,
namespaces, and attributes use `PascalCase`; values and functions use
`camelCase`; only constants use `UPPER_SNAKE_CASE`.

## Consequences

- Standard-library/API review checks arbitrary truncations and redundant
  namespace prefixes together rather than optimizing one at the cost of the
  other.
- Diagnostics offer context-aware expansions; ambiguous renames require review.
- Documentation and examples use the same canonical names as shipped APIs.
- New conventional initialisms require explicit design review instead of
  silently becoming a general abbreviation escape hatch.

## Alternatives considered

### Use `Iter` as the sequence namespace

Rejected because it overloads a truncation across distinct concepts and makes
the API less readable.

### Forbid every shortened technical word

Rejected because established names such as `Json`, `Http`, and `Async` are
clear domain vocabulary rather than project-local compression.

## Required conformance tests

- exact prelude/API snapshots for `Iterable`, `Iterator`, and `Sequence`;
- negative API tests for `Iter` and `iter.map`;
- naming diagnostics and context-aware rename-fix tests;
- casing tests for accepted technical initialisms and uppercase constants;
- API review tests that catch both truncation and redundant namespace prefixes.

## Documents/components affected

Syntax/nomenclature, `Pop.Standard`, diagnostics and quick fixes, formatter/style
tooling, XML documentation examples, API baselines, and architecture CI.
