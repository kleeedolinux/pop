# ADR 0005: Luau-First Surface Syntax

- Status: accepted
- Date: 2026-07-10

## Context

Pop Lang is inspired by Luau, but early illustrative class and module syntax
used forms resembling JavaScript and other languages. Examples influence later
parser and API decisions even when labeled provisional.

## Decision

Pop Lang starts with Luau's lexical, expression, annotation, function, and block
conventions. Native constructs add the smallest coherent extension and should
use `end`-delimited blocks, `function`, `local`, colon method ergonomics, and
Luau-like type annotation placement where applicable.

Files use a native file-scoped `namespace` plus semicolon-free `using` directives
for static name resolution; they do not copy JavaScript named-import/
destructuring syntax. Braces remain data/typed-initializer literals rather than
declaration blocks. D syntax is not imported along with the UDA concept.

## Consequences

- Luau programmers retain a familiar reading and editing model.
- Some new constructs still require keywords because Luau has no native class,
  namespace, Bubble, or Package declaration syntax.
- Syntax proposals must compare themselves with existing Luau conventions.
- Architecture examples remain provisional but must follow this direction.

## Alternatives considered

### Choose syntax independently for every feature

Rejected because it produces a language that is semantically coherent but
visually fragmented and less recognizably Luau-inspired.

### Preserve exact Luau grammar

Rejected because native classes, namespaces/Bubbles, UDAs, and explicit compile-time
facilities require additions that Luau does not provide.
