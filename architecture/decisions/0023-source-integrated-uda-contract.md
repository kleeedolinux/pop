# ADR 0023: Source-Integrated UDA Usage and Compile-Time Queries

- Status: accepted
- Date: 2026-07-10
- Supersedes: none

## Context

ADR 0004 establishes typed UDAs and restricted compile-time execution. The
bootstrap implementation exposed function attachments and constant evaluation,
but the architecture did not yet fix how a user attribute declares targets,
repeatability, validation, or typed queries in source.

## Decision

Attribute declarations remain one-line nominal typed declarations. Their usage
contract is supplied by trusted compiler attributes applied to the attribute
declaration:

```luau
@AttributeUsage(
    targets = { AttributeTarget.Record, AttributeTarget.Field },
    repeatable = false,
)
@AttributeValidator(validateSerializable)
public attribute Serializable(version: UInt32 = 1)
```

`AttributeTarget` is a closed compiler-owned enum covering each supported Item
kind. Omitted `@AttributeUsage` means one occurrence on namespace declarations
only; it never silently means unrestricted attachment. A validator is an
already-resolved `@CompileTime` function reference with a compiler-defined typed
signature. Runtime retention remains an explicit trusted `@RetainMetadata`
capability and is never implied by usage.

Attachments may precede declarations and class/record/union/interface members.
They preserve source order. A non-repeatable duplicate, wrong target,
inaccessible attribute, invalid argument, or failed validator is an error.
Unrecognized attachments are never silently discarded.

The source query form is the already documented Luau-direction generic call:

```luau
attribute<<Serializable>>(User)
```

The attribute type argument and symbol/type operand are resolved before
compile-time HIR. A non-repeatable query returns `Serializable?`; a repeatable
query returns an immutable `{Serializable}`. The operand is a compiler-owned
typed handle, never a string. The companion `hasAttribute<<A>>(symbol)` returns
`Boolean`. Version one exposes no enumeration or by-name query.

Compile-time functions support immutable locals, tuples, immutable records,
homogeneous immutable arrays, enum/union values, strings, and the opaque typed
handles accepted by ADR 0004. The verifier checks the complete recursive value
shape and handle kind/ownership. Cycles are diagnosed before evaluation.

Every evaluation publishes deterministic dependencies on functions, types,
symbols, attributes, and canonical arguments, plus fuel/depth/allocation/live-
value/output-size/diagnostic budgets and an origin/call chain. Compiler handles
and compile-time-only functions are rejected from runtime HIR/MIR.

## Consequences

- Attribute usage is explicit without adding declaration-block punctuation.
- Query APIs remain typed, visibility-preserving, and cacheable.
- Compiler-defined usage/validator identities cannot be spoofed by spelling.
- Fields/cases/methods need stable Item identities and attribute collections in
  HIR even when attributes erase before MIR.

## Alternatives considered

### Attribute declaration option blocks

Rejected because they add a second declaration mini-language and visual
ceremony to a lightweight metadata feature.

### Infer target permissions from where an attribute first appears

Rejected because it is order-dependent and cannot be represented reliably in
public metadata.

### Query by string name or enumerate all members

Rejected by ADR 0004 and the no-string-mixin/reflection boundary.

## Required conformance tests

- every target kind, repeatability, source order, validation, visibility, and
  retention boundary;
- record/array/enum/union/reference value canonicalization and structural
  verifier negatives;
- non-repeatable/repeatable typed query results and inaccessible-query errors;
- cycle, every resource limit, dependency invalidation, and provenance chains;
- cross-process/target determinism;
- handle/runtime escape, source parsing/injection, ambient I/O, FFI, backend
  access, global enumeration, and string lookup negative tests.

## Documents/components affected

UDA/compile-time architecture, syntax, type checker, compile-time interpreter,
query engine, HIR, driver, diagnostics, metadata artifacts, and conformance
tests.
