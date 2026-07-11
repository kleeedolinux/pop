# ADR 0013: Binding Architecture and No Lua Regression

- Status: accepted
- Date: 2026-07-10

## Context

The architecture is an initial foundation and will evolve, but “not final” can
be misread as permission for implementations to silently diverge. Pop Lang is
also Luau-inspired, which could be misused to restore Lua's dynamic/table-based
semantics under the name of compatibility.

## Decision

Accepted architecture is a binding baseline for implementation, specifications,
tests, examples, libraries, tools, and backends. A conflicting implementation is
an architecture bug until corrected or preceded by an accepted superseding ADR
and coordinated document/test updates.

Uncovered cross-cutting semantic/public behavior is an architecture gap and
cannot stabilize before design. Private implementation details remain free when
they preserve all contracts.

Reintroducing Lua's dynamic values, universal tables, metatable-based ordinary
classes, runtime `require` table modules, implicit globals/environments, runtime
string member lookup, untyped variadics, duck typing, or compatibility-over-
soundness is classified as a release-blocking Lua regression.

Luau-like readable syntax, functions, closures, coroutines, typed tables, and
ergonomics remain intentional when implemented through Pop Lang's static/native
architecture.

## Consequences

- Semantic/public changes need architecture authorization and traceable tests.
- Architecture gaps block stabilization instead of being decided accidentally in
  code.
- Experiments remain isolated until accepted through ADR review.
- CI gains architecture/prelude/IR/backend/Lua-regression gates.
- Documentation examples are held to the same direction as code.
- Performance or migration convenience cannot silently waive core identity.

## Alternatives considered

### Treat architecture as optional guidance

Rejected because the first convenient implementation would become accidental
semantics and backends/libraries would drift apart.

### Freeze architecture permanently

Rejected because implementation and real users will reveal better designs. The
correct model is binding but explicitly evolvable.

### Provide normal Lua-compatibility mode

Rejected because dynamic/table semantics would contaminate the type system, IR,
runtime, libraries, diagnostics, and user expectations. Migration tooling and
explicit interop are safer boundaries.

