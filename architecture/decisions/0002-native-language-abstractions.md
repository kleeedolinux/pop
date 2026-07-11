# ADR 0002: Native Classes and Modules

- Status: accepted
- Date: 2026-07-09

## Context

Lua's tables provide a compact substrate for objects, classes, namespaces, and
modules. Luau adds strong tooling and types but retains many table-centered
foundations. Pop Lang wants Luau-like ergonomics with clearer semantics and more
predictable native compilation.

## Decision

Classes, namespaces, Modules, and Bubbles are first-class language and
compiler concepts. Records, tuples, arrays, and tables are distinct
semantic categories. Ordinary class instances cannot acquire undeclared fields,
and public declarations cannot be discovered or changed through table indexing.

Tables remain supported statically typed associative collections. Heterogeneous
content requires an explicit union or interface. Reflection and operator
customization use explicit restricted facilities rather than changing the
meaning of normal classes or modules.

## Consequences

- Declared fields can use fixed layouts and direct access.
- Module dependency and initialization order can be analyzed statically.
- Migration from table-heavy Luau sometimes requires explicit refactoring.
- The language needs real rules for construction, visibility, dispatch, cycles,
  and reflection.
- The runtime may still share internal implementations when semantics permit,
  but those details are not observable language equivalences.

## Alternatives considered

### Keep tables as the universal substrate

Rejected because it preserves ambiguous semantics, weakens static guarantees,
and makes predictable layouts and module analysis harder.

### Remove tables entirely

Rejected because associative collections and Luau-like table literals are useful
parts of the programming model Pop Lang wants to preserve. Static key/value
types provide that capability without dynamic values.
