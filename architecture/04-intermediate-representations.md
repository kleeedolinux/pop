# Intermediate Representations

## Representation boundaries

| Representation | Primary purpose | May contain | Must not contain |
| --- | --- | --- | --- |
| Syntax tree | Lossless source model | tokens, trivia, recovery nodes | inferred types, layouts |
| Resolved AST | Bound source model | symbols, modules, UDA syntax | LLVM details, dynamic lookup |
| Compile-time HIR | Restricted typed evaluation | constants, UDA values, type/symbol handles | parsing, runtime reflection |
| HIR | Typed language semantics | classes, attributes, patterns, typed expressions | parser recovery, LLVM values |
| MIR | Portable execution semantics | CFGs, typed values, abstract runtime operations | source sugar, LLVM opcodes |
| C11 source | Experimental backend artifact | exact-width C types, checked helpers, private control-flow lowering | canonical language semantics, unchecked fallbacks |
| LLVM IR | Native backend implementation | LLVM types, intrinsics, target ABI | canonical language semantics |

## Stable identities

Compiler entities use explicit IDs rather than pointers as public identity:

- `WorkspaceId`, `PackageId`, `BubbleId`, `ModuleId`, `FileId`, and `SpanId`;
- `SymbolId`, `TypeId`, `ClassId`, `AttributeId`, and `FunctionId`;
- `InterfaceId`, `InterfaceMethodId`, and `CaptureId`;
- `BlockId`, `ValueId`, `SafePointId`, and `StackMapId` in MIR.

IDs can be dense within a compilation session. Serialized caches pair them with
stable definition/content keys rather than persisting raw session-local numbers.

## HIR

HIR is source-oriented and backend-independent. Every expression has a resolved
static type and source span. Every call identifies a typed dispatch category.

Representative concepts:

```text
HirBubble {
  id, bubbleId, namespaceId, usingBindings, bubbleDependencies,
  declarations, publicSymbols, attributes
}

HirDeclaration {
  symbolId, visibility: Public | Internal | Private,
  kind, type, attributes, origin
}

HirClass {
  id, typeParameters, base, interfaces, fields, methods, attributes
}

HirInterface {
  id, methods{interfaceMethodId, signature, effects}, attributes
}

HirExpr =
  Literal | Local | Assign | Block | If | Loop | Match
  | Function | Closure{functionId, captures} | Call{dispatch, effects}
  | Construct{classId}
  | FieldGet{fieldId} | FieldSet{fieldId}
  | Record | RecordUpdate | Tuple | Array | Table | Convert{kind}
  | Return | Break | Continue | Await
```

This is conceptual notation, not a commitment to an implementation language.

HIR invariants:

- every name is resolved or represented by an explicit diagnostic-recovery node;
- every expression has a type, including internal `Error` and `Never` types;
- field and method accesses identify a member or typed dispatch slot;
- calls exactly match the resolved callable signature, receiver, results, and
  effect summary;
- closures identify every capture, its type, owner, and value/cell mode;
- interface calls identify the static interface and resolved member/slot;
- matches name every case of one resolved tagged union exactly once;
- implicit source conversions are explicit HIR conversion nodes;
- source spans survive desugaring through origin chains;
- no target word size is assumed for language-defined numeric types;
- no valid HIR node means “perform this operation dynamically.”
- every namespace-scope declaration has resolved visibility; `publicSymbols` is
  derived from declarations and is not a source-level export list;
- every item and source origin identifies its owning `ModuleId` and `BubbleId`.
- a `repeat` statement retains its typed body and `Boolean` exit condition until
  CFG lowering; its body-local scope includes that condition only;
- a numeric `for` retains its immutable binding, same-kind integer bounds,
  optional step, and body, while `break`/`continue` retain their resolved
  innermost-loop target until CFG lowering;
- a conditional expression retains one `Boolean` condition and two same-typed
  lazy branches until MIR lowers them to CFG and a typed join argument;
- compound assignment retains its resolved mutable target, typed operator, and
  right-hand side until lowering can evaluate a receiver/index once and emit
  ordinary MIR load-operation-store instructions.
- a fixed result pack retains exact element types; grouped multiple assignment
  retains resolved targets until MIR emits target locations, values, typed
  projections, and stores in the order fixed by ADR 0045.

## Compile-time HIR and values

Compile-time evaluation reuses typed expressions but has a smaller effect and
capability set. Values include ordinary immutable constants and compiler-owned
handles such as `TypeRef`, `SymbolRef`, `FieldRef`, and `AttributeValue`.

Compiler handles are opaque, session-local, and cannot be constructed from
strings or integers. The compile-time interpreter can:

- call functions accepted by the compile-time effect checker;
- allocate bounded immutable compile-time data;
- query UDAs on an accessible symbol;
- request a deliberately small set of facts through typed handles;
- produce constants and structured compile-time diagnostics.

It cannot parse source, construct tokens, inject declarations, inspect LLVM,
access runtime heap state, bypass visibility, or turn a member-name string into
a symbol. Compiler handles never lower to MIR and are never serialized as
runtime values.

## MIR

MIR is a typed control-flow graph organized by functions and basic blocks. Its
type system is smaller than the source type system. Nominal and generic facts
remain only where required for semantics, optimization, layout, or debugging.

Representative operation families:

```text
Control:       branch, condBranch, switch, return, trap, panic, resumeUnwind,
               unreachable
Values:        const, tupleMake, tupleGet, recordMake, fieldGet, fieldSet
Arithmetic:    checkedAdd, wrappingAdd, floatAdd, compare, convert
Memory:        allocateObject, allocateClosureEnvironment, allocateArray,
               load, store, captureLoad, captureStore, retainRoot, releaseRoot
Calls:         callStandard{standardFunctionId}, callDirect, callVirtual,
               callInterface, callIndirect
Types:         typeTest, checkedDowncast, makeUnion, projectUnion
Collections:   arrayCreate, arrayLength, arrayGetOptional, arrayGetChecked,
               arraySet, arrayFill, tableGet, tableSet
Runtime:       gcSafePoint{stackMap}, writeBarrier, pin, unpin, suspend, resume
Debug:         debugValue, sourceScope
```

`checkedDowncast` has a named static target and typed optional/result output. It
does not create an untyped value. Collection operations carry concrete key,
value, and collection types.

Numeric conversion operations carry exact source and target integer/float
kinds. Checked integer-target conversions name `NumericConversion`; float
ordering uses ordered comparisons so NaN does not make `<=`/`>=` true. These
operations never accept a runtime type name or defer conversion selection to a
backend. See ADR 0040.

Array construction always carries an explicit initial value. Checked reads and
writes carry `BoundsViolation`; optional reads do not trap for bounds. Scalar
and managed-element arrays remain distinguishable for optimization and precise
barriers. See ADR 0034.

MIR invariants:

- each block has one terminator;
- each value dominates its uses, or arrives as a block argument;
- operand and result types are valid for the operation;
- control-flow edges pass declared block arguments;
- potentially failing operations have explicit trap/unwind/result semantics;
- calls declare effects relevant to optimization and safe points;
- call effects are a known subset of the caller's declared effects;
- a panic-capable call explicitly propagates unwind or names a cleanup block;
- stack maps contain exactly the live managed values at each safe point and
  logical object maps contain exactly the managed fields of allocations;
- a collecting safe point may change the physical token for every live managed
  value; backends/VMs install the typed `RootSlot` updates before subsequent
  uses without adding backend relocation instructions to canonical MIR;
- root scopes dominate their uses and are balanced on normal and unwind exits;
- evaluation order matches Pop Lang semantics;
- all target assumptions come from target queries;
- every call and member/collection operation has statically known types;
- no instruction performs name lookup or type discovery from a runtime string;
- MIR verification runs after construction and every transforming pass.

Body-first loops lower to ordinary CFG body, condition, exit, and backedge
blocks. They do not introduce a backend-specific instruction; the verifier
requires the same deterministic safe-point treatment as every other backedge.

String concatenation and primitive formatting remain typed in HIR. Canonical
MIR uses backend-neutral `StringConcat` and `StringFormat` operations, verifies
the exact operand kind, and records their allocation and safe-point effects.
Interpolation lowers in source order through those operations; it never becomes
a runtime format string, type inspection, or backend-specific instruction. See
ADR 0041.

Fixed type packs lower to one typed tuple-like MIR value. `tupleMake` constructs
an exact pack and `tupleGet` projects a statically indexed element; grouped
multiple assignment then uses ordinary stores and barriers. MIR contains no
dynamic variadic carrier, runtime arity adjustment, or comma semantics. See ADR
0045.

The initial portable failure/GC encoding is fixed by ADR 0022. Runtime traps
are closed `TrapKind` values and are not ordinary exceptions. Panic uses a
runtime-private typed payload. Expected failures continue to use typed result
values. ADR 0040 adds `NumericConversion` for checked numeric casts that receive
NaN, infinity, or an out-of-range value. ADR 0042 adds `InvalidRangeStep` for a
dynamic zero step in a numeric `for` range.

## Attribute representation

A resolved UDA is stored as its attribute type plus a canonical immutable value
and origin span. Attribute values may contain primitives, enums, type/symbol
handles, tuples, and immutable records/arrays accepted by the compile-time type
system. Runtime objects, closures with mutable state, raw pointers, and backend
handles are forbidden.

HIR owns compile-time attributes. MIR sees only semantic consequences already
resolved by the front end or explicit retained-metadata constants.

## Abstract layouts

HIR knows semantic fields but no byte offsets. MIR can request an abstract layout
for a type/class and refer to logical fields. The backend layout service chooses
offsets, alignments, stack locations, and calling conventions.

Primitive widths are language-defined where observable. Target-sized types, if
offered, are explicitly named. A generic integer name cannot quietly change
width between backends unless that behavior is specified by the language.

## Serialization and textual form

HIR and MIR have deterministic textual dump formats from the first milestone.
MIR may later gain a versioned binary form for caching and VM tooling. Dumps are
test formats, not automatically stable public APIs.

Every MIR fixture should parse back into the verifier. This enables backend tests
without invoking the Pop Lang parser or compile-time engine.

## Pass manager

Passes declare required/preserved analyses, control-flow effects, accepted MIR
stage, determinism, thread safety, and verification requirements. A backend
accepts only documented canonical MIR, never construction MIR.
