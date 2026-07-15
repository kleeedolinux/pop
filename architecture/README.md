# Pop Lang Architecture

This directory is the architectural source of truth for **Pop Lang**.

Pop Lang is a native, strongly and statically typed language inspired directly
by Luau. It keeps Luau's lightweight syntax, local type inference, first-class
functions, familiar control flow, table literals, coroutines, and focus on fast
tooling. It deliberately leaves behind Lua's use of tables as the hidden
implementation of classes, modules, records, and every other abstraction.

Pop Lang is not an object-oriented-first language. Programs are expected to use
records, tagged unions, plain functions, namespaces, generic algorithms, and
composition for most work. Native classes exist for the narrower cases that
need identity, encapsulated mutable state, inheritance, or runtime dispatch.

Pop Lang has no dynamically typed values. Inference may save the programmer
from writing a type, but the compiler must determine a concrete static type for
every value and operation. Native classes, modules, records, tuples, arrays,
and typed tables have explicit semantics.

The compiler is designed around backend-independent HIR and MIR. LLVM IR is one
backend representation, not the compiler's internal truth. This boundary lets
Pop Lang add a custom VM later without replacing the front end, type checker,
compile-time evaluator, or portable optimizer.

## Documents

1. [Vision and principles](./01-vision-and-principles.md)
2. [Language model](./02-language-model.md)
3. [Compiler pipeline](./03-compiler-pipeline.md)
4. [Intermediate representations](./04-intermediate-representations.md)
5. [Runtime and ABI](./05-runtime-and-abi.md)
6. [Backend architecture](./06-backend-architecture.md)
7. [Implementation roadmap](./07-implementation-roadmap.md)
8. [Open design questions](./08-open-design-questions.md)
9. [Closed design questions](./08.1-closed-design-questions.md)
10. [Relationship to Luau](./09-relationship-to-luau.md)
11. [UDAs, compile time, and reflection](./10-udas-compile-time-and-reflection.md)
12. [Compiler component architecture](./11-compiler-component-architecture.md)
13. [Type-system architecture](./12-type-system-architecture.md)
14. [Syntax and nomenclature](./13-syntax-and-nomenclature.md)
15. [Bubbles, namespaces, artifacts, and loading](./14-libraries-namespaces-and-loading.md)
16. [Garbage collector architecture](./15-garbage-collector-architecture.md)
17. [Base libraries](./16-base-libraries.md)
18. [Diagnostics, warnings, and quick fixes](./17-diagnostics-warnings-and-quick-fixes.md)
19. [Paradigm and API style](./18-paradigm-and-api-style.md)
20. [Architecture conformance and regression policy](./19-architecture-conformance-and-regression-policy.md)
21. [XML documentation comments](./20-xml-documentation-comments.md)
22. [CLI, tooling, and units of code](./21-cli-tooling-and-code-units.md)
23. [Public standard-library architecture](./22-public-standard-library-architecture.md)
24. [Core and portable library catalog](./22.1-core-and-portable-library-catalog.md)
25. [System, network, and security catalog](./22.2-system-network-security-catalog.md)
26. [Data, observability, and tooling catalog](./22.3-data-observability-tooling-catalog.md)
27. [Application, media, and science catalog](./22.4-application-media-science-catalog.md)
28. [Public library API examples](./22.5-standard-library-api-examples.md)
29. [Public library implementation plan](./22.6-standard-library-implementation-plan.md)
30. [Concurrency, actors, and distribution](./23-concurrency-actors-and-distribution.md)
31. [Scheduler runtime implementation](./23.1-scheduler-runtime-implementation.md)

The examples define the canonical syntax direction. The full grammar will grow
with implementation, but `.pop`, the `pop` command, naming rules, namespace/
`using` headers, and Luau-shaped block style are accepted decisions. New syntax
must look like a natural extension of Luau, not JavaScript, Rust, or D
transplanted into a Luau-shaped file.

## Architectural invariants

- Every runtime value and operation has a statically proven type.
- There is no `dynamic`, `any`, dynamically typed escape hatch, or dynamic
  member/call operation.
- Type inference never weakens static checking.
- HIR and MIR never contain LLVM objects, LLVM opcodes, or LLVM data layouts.
- Source constructs are resolved and type-checked before reaching MIR.
- MIR has explicit control flow, calls, lifetime effects, and failure edges
  wherever those details affect code generation.
- Language semantics are identical across conforming backends.
- The experimental C11 backend consumes optimized verified MIR, preserves
  checked semantics without C undefined behavior, and rejects runtime features
  outside its declared capability set.
- Classes, modules, records, tuples, arrays, and tables are distinct
  concepts even when an implementation can share storage internally.
- Runtime services are reached through a versioned backend-neutral interface.
- Runtime implementation ownership is split among the pure PLRI contract, the
  portable collector, the native ABI vocabulary, and the native host facade;
  native symbols and platform state never enter PLRI.
- User-defined attributes contain typed compile-time values.
- Compile-time evaluation cannot parse or inject source strings.
- Reflection is compile-time-first, visibility-preserving, capability-limited,
  and absent from runtime artifacts unless explicitly requested.
- Namespaces/types/attributes use `PascalCase`; values/functions use `camelCase`;
  only constants use `UPPER_SNAKE_CASE`.
- `using` changes compile-time name resolution and never loads code.
- Code ownership is `Item → Module → Bubble → Package → Workspace`; a Bubble is
  the independent crate-like compilation and `internal` boundary.
- Packages use `bubble.toml`; Workspaces share deterministic `bubble.lock`
  resolution and a `target/` cache without sharing visibility.
- Library Bubbles emit self-describing `.poplib` artifacts resolved by
  `BubbleIdentity` and the locked dependency graph.
- `bubble.lock` and `.poplib` control files use bounded canonical JSON; artifact
  integrity uses normalized SHA-256 file inventories under ADR 0055.
- Pop GC uses precise roots, a moving nursery, and concurrent mature marking.
- Collecting safe points update typed `RootSlot` publications in place; object
  identity survives relocation while stale physical reference tokens do not.
- Source-visible built-in types and attributes use `PascalCase`, including
  `String`, `Int`, `Boolean`, `UInt32`, and `@Serializable`.
- `Pop.Internal` is the trusted private compiler/runtime library;
  `Pop.Standard` is the native Pop portable public foundation.
- Public-library APIs optimize for short direct call sites and explicit cost;
  convenience cannot hide allocation, copying, dispatch, authority, or native
  transitions.
- The `Pop` prelude is fixed, curated, and automatically available; ordinary
  standard-library use requires no `using` directives.
- Namespace declarations use explicit `public`, `internal`, or `private`
  visibility; Pop Lang has no `export` keyword/list.
- Public names use complete words. Arbitrary truncations such as `Iter` are
  forbidden; standardized initialisms remain word-cased (`Json`, `Http`, `Utf8`).
- Records/functions/composition are preferred over class hierarchies and fluent
  object APIs.
- Diagnostics have stable `POP####` identities and structured safe quick fixes.
- Expected failures use the reserved typed `Result` union, nominal error
  declarations, exact prefix propagation, exhaustive recovery, and lexical
  last-in, first-out cleanup.
- Generalized iteration uses the reserved nominal `Iterable<T>`, `Iterator<T>`,
  and `Iteration<T>` contracts; arrays remain fixed and `List<T>` owns
  sequential growth.
- Generic calls infer one complete canonical argument list or fail statically;
  nominal bounds and verified portable specialization capsules preserve exact
  types and Bubble identity across dependency boundaries.
- Source overloads select one non-generic function by exact argument type pack;
  conversions, result context, declaration order, and runtime values never
  participate.
- Every normal Bubble receives the verified reserved `Pop.Standard` reference;
  package linking consumes the target implementation reloaded from `.poplib`.
- Lua-shaped `---` XML documentation comments are parsed, signature-checked,
  emitted with library metadata, and available to editors/doc tools.
- Non-empty XML documentation elements always use separate opening, body, and
  closing `---` lines; the formatter enforces the deterministic
  [ADR 0057](./decisions/0057-multiline-xml-documentation-format.md) form.
- Accepted architecture is a binding baseline. Undocumented semantic expansion
  or implementation divergence is a bug until an ADR changes the baseline.
- Reintroducing Lua's dynamic/table-centered architecture is a release-blocking
  **Lua regression**, not a compatibility feature.
- A future VM can consume MIR without reconstructing source-level meaning.
- Native FFI declarations remain ordinary statically typed functions; typed
  hashed link plans, exact ABI layouts/effects, scoped pins/handles/callbacks,
  and generated reviewable adapters replace raw flags, shell execution,
  runtime symbol lookup, or reflection.
- The initial compiler/runtime/tool implementation uses a Rust 2024 virtual
  Cargo workspace with architecture-tested crate dependency boundaries.

## Decision process

Changes that alter syntax alone belong in the language specification. Changes
that affect several compiler layers, observable semantics, compile-time
execution, metadata retention, object layout, the runtime ABI, or backend
portability require an Architecture Decision Record under
`architecture/decisions/`.

“The architecture is not final” means it can be changed deliberately. It does
not mean implementations may ignore it. An accepted ADR and dependent document/
test updates must precede a conflicting implementation. See
[Architecture conformance and regression policy](./19-architecture-conformance-and-regression-policy.md).
