# Relationship to Luau

Pop Lang intends to account for Luau's useful language features, not to clone
its compatibility constraints. “Everything from Luau” therefore means **no
silent omissions**: each relevant Luau feature must be adopted, adapted,
replaced, rejected with a reason, or deliberately deferred.

This is an architectural inventory, not yet a complete compatibility table.

## Classification

| Class | Meaning |
| --- | --- |
| Adopt | Preserve the programmer-facing concept with closely related semantics. |
| Adapt | Preserve the capability, but fit it to Pop Lang's native object/type model. |
| Replace | Offer a clearer first-class construct for the same use case. |
| Reject | Exclude it with a documented semantic, safety, or complexity reason. |
| Defer | Keep it out of the initial language until its interactions are designed. |

## Initial feature direction

| Luau area | Pop Lang direction | Class |
| --- | --- | --- |
| Gradual typing and local inference | Keep inference and familiar annotations; reject gradual fallback. Every successful value is statically typed. | Adapt |
| Type annotations, aliases, generics, unions, intersections, singleton types, and narrowing | Core type-system input with soundness rules and no `any` operations. | Adopt / Adapt |
| Type packs and variadic results | Use fully typed tuples/type packs and preserve ergonomic destructuring. | Adapt |
| First-class functions and closures | Core feature with explicit closure conversion in MIR. | Adopt |
| Coroutines | Preserve structured suspension capability behind backend-neutral MIR/runtime operations. | Adapt |
| `repeat`/`until`, `continue`, compound assignment, if-expressions, generalized iteration, and string interpolation | Preserve them in the source language with canonical formatting; ADR 0032 fixes the body-first loop contract, ADR 0042 fixes numeric `for` ranges and loop control, ADR 0043 fixes conditional expressions, and generalized iteration remains protocol-gated. | Adopt |
| Immutable/`const` bindings and attributes | Preserve them; attributes extend to typed PascalCase UDAs. | Adapt |
| Tables | Keep Luau-like literal/indexing ergonomics, but require static key/value types. | Adapt |
| Table types used as records or objects | Replace with structural records, nominal classes, interfaces, typed tables, and arrays. | Replace |
| Metatables and metamethods | Replace common use cases with classes, interfaces, and typed operator/iteration protocols. | Replace / Defer |
| `require`-style module values | Replace with file-scoped namespaces, direct namespace declarations, `bubble.toml` dependencies, Bubble metadata, and analyzable initialization. | Replace |
| Implicit globals and mutable environment functions | Exclude; unsafe facilities must still expose statically typed operations. | Reject |
| Single IEEE-754 `number` type | Replace with explicit PascalCase fixed-width integers/floats; `Int` is `Int64` and `Float` is `Float64`. | Replace |
| Lua-compatible standard library details | Replace with the native, tiered `Pop.Standard` architecture; provide migration adapters only when they do not distort semantics. | Adapt / Replace |
| Sandboxed embedding | Preserve as a deployment capability, not as a reason to couple the language to one host. | Adapt |
| Attributes | Keep Luau's lightweight `@` direction and extend it with typed user-defined attributes evaluated at compile time. | Adapt |
| `any`/gradual escape behavior | Reject runtime operations without a proven static type. Migration requires schemas, unions, interfaces, or typed wrappers. | Reject |

## Compatibility workflow

Before declaring the initial language feature-complete, maintain a detailed
inventory against the then-current Luau grammar, type system, standard library,
and implemented RFCs. Every row receives one classification above, a Pop Lang
specification link, and conformance or migration tests.

This workflow is intentionally versioned. Luau continues to evolve, so “Luau
coverage” always records the reviewed Luau version or commit and date.

## Design rule

Familiar syntax is preferred when semantics match. When Pop Lang has a native
concept with stronger guarantees, its syntax should make that distinction clear
instead of preserving a table idiom solely for source compatibility.

Reintroducing Lua semantics through compatibility is an architectural bug. The
permanent boundary and forbidden regression list live in
[Architecture conformance and regression policy](./19-architecture-conformance-and-regression-policy.md).

## Research references

Feature names in this initial inventory were checked on 2026-07-09 against the
official [Luau syntax documentation](https://luau.org/syntax/), [type-system
documentation](https://luau.org/types/), [grammar](https://luau.org/grammar/),
and [Lua compatibility notes](https://luau.org/compatibility/). These links are
research inputs, not normative sources for Pop Lang.
