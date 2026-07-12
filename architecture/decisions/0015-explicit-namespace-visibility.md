# ADR 0015: Explicit Namespace Visibility

- Status: accepted
- Date: 2026-07-10
- Superseded in part by: ADR 0029 default internal visibility and empty results

## Context

Pop Lang namespaces are compile-time scopes, not runtime tables or module
objects. An `export` prefix/list would add a second declaration mechanism and
would encourage JavaScript/Lua-style module containers. Requiring utility
classes merely to group functions would push a procedural, data-oriented
language toward unnecessary OOP.

## Decision

Every namespace-scope declaration has exactly one resolved visibility:
`public`, `internal`, or `private`. ADR 0029 makes omitted visibility resolve
to `internal` except for the binary-root `main` shorthand defined by ADR 0026.
Pop Lang has no `export` keyword, export list, or re-export operation.

- `public` is visible to dependent Bubbles and appears in reference metadata.
- `internal` is visible to all Modules in the declaring Bubble and is
  absent from its public reference surface.
- `private` is visible only in the declaring Module/file.
- `local` remains the block/function-local binding keyword and is not a
  namespace visibility.

Functions, constants, types, and attributes may be declared directly in a
namespace. A namespace has no runtime value and does not require a static class,
singleton, or returned table. A `using` changes lookup only; it cannot change a
declaration's visibility.

Record fields and union/enum cases are part of their containing contract.
Interface members are public by definition. Class members explicitly state
visibility; `protected` is not part of the initial language.

## Consequences

- The parser assigns default internal visibility where the modifier is omitted
  and rejects `export`.
- Declaration indexes, symbols, HIR, documentation, and metadata carry the
  visibility enum directly.
- Reference metadata includes only public declarations and compile-time facts
  required by those declarations.
- Namespace functions remain concise while API boundaries stay obvious.
- Quick fixes can add a missing modifier or migrate an obsolete draft `export`
  prefix to `public` when that intent is unambiguous.

## Alternatives considered

### Keep `export`

Rejected because it duplicates visibility, resembles module-object systems,
and makes `internal`/`private` design less coherent.

### Use implicit default visibility

Superseded by ADR 0029. Default `internal` does not expose a public API and
keeps Bubble-local declarations concise.

### Put namespace functions in static utility classes

Rejected because namespaces already organize names and Pop Lang avoids OOP
ceremony where data and functions express the design.

## Required conformance tests

- parser tests for all three modifiers and default internal visibility;
- resolver tests for file-private and Bubble-internal boundaries;
- reference-metadata tests proving only public declarations are exposed;
- HIR snapshots carrying explicit visibility without export lists;
- direct namespace-function calls lowering to resolved direct calls;
- negative tests for static utility containers and module return tables in the
  standard API baseline.

## Documents/components affected

Syntax, declaration indexing, resolution, HIR, library metadata, documentation,
diagnostics, language-server completion, API baselines, and conformance policy.
