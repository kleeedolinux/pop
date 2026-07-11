# ADR 0006: Source Naming and Aesthetic

- Status: accepted
- Date: 2026-07-10

## Context

Pop Lang should retain Lua/Luau's visual simplicity while adding native classes,
namespaces, Packages/Bubbles, UDAs, and compile-time programming. Inconsistent casing
and syntax borrowed feature-by-feature would make the language feel noisy.

## Decision

Namespaces, Packages, Bubbles, all types including built-ins, interfaces, enum cases,
type parameters, and UDAs use `PascalCase`. Functions, methods, fields, locals,
parameters, modules, and filenames use `camelCase`. Only constants use
`UPPER_SNAKE_CASE`.

Source blocks use Luau-style keywords and `end`, not braces or semicolons.
Compiler attributes use `PascalCase`. The formatter defines one canonical calm
layout.

## Consequences

- The parser/style checker can diagnose noncanonical source names.
- Existing architecture examples and future standard-library APIs follow one
  convention.
- Acronyms behave as words (`HttpRequest`, `parseJson`).
- Lowercase `snake_case` is unavailable for ordinary names.

## Alternatives considered

### Follow Luau's permissive naming style

Rejected because a standard convention improves API coherence and fulfills Pop
Lang's stronger language/tooling identity.

### Use PascalCase only for types and allow mixed function styles

Rejected because mixed project conventions would quickly fragment library APIs.
