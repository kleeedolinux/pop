# Vision and Principles

## Product statement

Pop Lang is a compact, native, strongly typed programming language for people
who like Luau and want it to grow beyond Lua's table-centered foundations.

“Inspired by Luau” is a surface-language and usability commitment. Ordinary Pop
Lang should read like Luau with coherent native additions. It is not permission
to import JavaScript module syntax, Java/C# ceremony, Rust punctuation, or D
template and string-mixin syntax.

Pop Lang is not Luau-compatible at any cost. Familiarity wins when semantics
match; stronger Pop Lang semantics win when compatibility would weaken the type
system, object model, compile-time safety, or backend independence.

## Goals

### Luau-first language feel

Pop Lang should feel immediately understandable to a Luau programmer:

- `local`, `function`, `if`/`then`/`else`/`end`, `while`, `for`, `repeat`, and
  familiar expression syntax;
- local type inference plus Luau-style `name: Type` annotations;
- first-class functions, closures, lexical scope, and multiple assignment;
- table literal and indexing syntax where the value is statically a table;
- `continue`, compound assignment, if-expressions, generalized iteration, and
  string interpolation;
- coroutine syntax and semantics built on backend-independent primitives;
- short, actionable diagnostics and fast incremental analysis.
- checked `---` XML documentation with editor hover, signature help, and safe
  quick fixes.

New constructs such as `class`, `namespace`, `using`, attributes, and compile-
time evaluation should add the least syntax necessary and retain Luau's block
and declaration style.

### Strong static typing

Every value has a compile-time type, whether written or inferred. There is no
`dynamic` or `any` value type and no unchecked member lookup or call. Unions,
generics, interfaces, opaque handles, and foreign values remain statically typed.

External untyped data enters through a parser, decoder, schema, or explicitly
unsafe typed FFI—not through a dynamic language value that spreads uncertainty
through the program.

### Native language abstractions

Features with distinct meaning receive distinct compiler and runtime models:

- a class is a nominal type with declared storage and methods;
- a Module is one file and the `private`/initialization boundary;
- a namespace organizes public/internal/private declarations across source
  modules;
- namespace-scope functions are first-class declarations with explicit
  `public`, `internal`, or `private` visibility;
- a Bubble is the independently compiled `internal`, reference, and loading
  boundary;
- a Package with `bubble.toml` versions/distributes one or more Bubbles;
- a Workspace coordinates Packages without merging their visibility;
- a record is typed structural data;
- a tuple is a fixed-size product value;
- an array is an indexed homogeneous sequence;
- a table is a statically typed key/value collection;
- an interface describes behavior without pretending to be a table.

### Not object-oriented by default

Pop Lang supports native classes without making every design a class hierarchy.
The default vocabulary is:

- records and tagged unions for data;
- plain functions for transformations and operations;
- namespaces/Modules for organization;
- generic algorithms for reusable behavior;
- composition for assembling capabilities;
- small nominal interfaces only where runtime/static polymorphism is needed;
- classes only for meaningful identity or encapsulated mutable lifecycle.

There is no universal `Object`, no requirement that functions live inside
classes, no static utility classes, and no standard-library design based on deep
inheritance. Method syntax is convenience, not the dominant abstraction model.

Pop Lang has no `export` keyword/list. Namespace visibility is expressed directly
on each declaration. Names use complete readable words rather than arbitrary
truncations; namespace context prevents repetition without shortening words.

### Safe compile-time programming

Pop Lang supports typed user-defined attributes and deterministic compile-time
evaluation. Compile-time code operates on typed values and restricted symbol
descriptors. It cannot generate, parse, or inject program text. String mixins,
`eval`, and equivalent source-text substitution are not part of the language.

### Restricted reflection

Compile-time metadata queries are precise and visibility-aware. Runtime
reflection is omitted by default, never provides untyped field values, and can
only exist through explicit metadata retention with narrow generated adapters.
The first release accepts only ADR 0096 typed codec schemas, whose canonical
descriptor is `.popc` and whose generated `Codec.Schema<T>` value is resolved
statically rather than through a registry.

### Multiple execution engines

The initial production backend may lower MIR to LLVM IR for native code. The
architecture also permits a bytecode VM, interpreter, JIT, WebAssembly backend,
or testing backend without duplicating parsing, type checking, or compile-time
analysis.

### Predictable performance

Declared class fields use resolved storage, typed calls use direct or explicit
interface/virtual dispatch, and collection operations have known key/value
types. Reflection metadata and unrelated runtime type machinery are not emitted
when a program does not require them; private metadata needed for GC or dispatch
is not exposed as language reflection.

### Coherent base libraries and tooling

The trusted `Pop.Internal` library owns compiler/runtime primitives while the
public `Pop.Standard` library provides Pop's native portable foundation. Its
capability breadth, tiers, concise API rules, and cost contracts are defined in
[Public standard-library architecture](./22-public-standard-library-architecture.md).
Stable structured diagnostics and semantic quick fixes are designed with the
language, not added after compilation works. The fixed `Pop` prelude makes
common types/functions available without repetitive imports.

The public library is judged by call sites as well as type structure. Common
work should take one or a few direct calls, while explicit views, buffers,
streams, options, and resource scopes preserve advanced control. Convenience
does not authorize hidden allocation, copying, dynamic dispatch, ambient
authority, or native transitions.

## Non-goals

- Full source or behavioral compatibility with Lua or Luau.
- Gradual typing or dynamically typed runtime values.
- Implementing classes and modules as standard-library table patterns.
- Treating every value as an object or every operation as a method.
- Deep framework inheritance hierarchies and factory/service class ceremony.
- Treating accepted architecture as optional implementation guidance.
- Restoring Lua's dynamic/table/metatable/module semantics as compatibility.
- General runtime member lookup by string.
- Unrestricted runtime reflection or a global registry of all program types.
- String mixins, compile-time `eval`, or text-to-source generation.
- Exposing LLVM types, calling conventions, or intrinsics as core semantics.
- Exposing the selected collector's headers, regions, or barriers as source
  language semantics.
- Making the first compiler self-hosting.
- Baking one registry vendor, cache implementation, or editor protocol into
  language semantics. The standard `pop` workflow remains replaceable through
  documented manifest, metadata, diagnostic, artifact, and registry contracts.

## Compatibility policy

Compatibility is divided into explicit layers:

1. **Familiarity:** common Luau expressions and control flow should look and
   behave similarly when Pop Lang has the same concept.
2. **Migration:** tooling may translate a useful Luau subset and report where
   gradual typing, implicit globals, metatables, or table-based objects require
   explicit Pop Lang types and constructs.
3. **Interop:** a future bridge may exchange schema-checked values or call typed
   APIs across a boundary.
4. **Conformance:** Pop Lang does not claim arbitrary Luau programs are valid
   Pop Lang programs.

## Design priorities

When principles conflict, use this order:

1. sound, explainable static semantics;
2. Luau-like readability and ergonomics;
3. actionable diagnostics and tooling;
4. restricted, deterministic compile-time behavior;
5. backend independence;
6. predictable performance;
7. convenience for compiler implementation.

An implementation shortcut cannot outrank these priorities. If a desired change
conflicts with accepted architecture, the architecture must be deliberately
revised first; silent drift is a bug.
