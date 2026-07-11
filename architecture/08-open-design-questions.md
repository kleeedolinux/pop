# Open Design Questions

This is the original question ledger. Implementations must use the accepted
answers rather than choosing locally.

The initial answers were accepted on 2026-07-10 in
[Closed design questions](./08.1-closed-design-questions.md). This file remains
as the question ledger. A question is open only if the closed-design document
explicitly marks its answer provisional or a later ADR reopens it.

## Language semantics

1. Is `Int` a fixed-width type, an arbitrary-precision type, or an inferred
   family of explicit integer widths?
2. Does Pop Lang retain a distinct `nil`, and can every reference hold it, or is
   nullability expressed only through optional types?
3. Are classes single-inheritance, composition-only, or paired with traits?
4. Are interfaces nominal, structural, or both in different positions?
5. Which Lua/Luau metamethod-like protocols deserve explicit operator traits?
6. Are multiple returns represented by tuples, type packs, or a combination
   with fully static variadic tails?
7. What are the exact equality, identity, and hashing rules for each value kind?
8. Are mutable globals allowed, and how do they interact with module cycles?

## Types and generics

1. Are generics reified, erased with dictionaries, monomorphized, or hybrid?
2. Can structural records be width-subtyped?
3. How are mutable collection types kept sound under variance?
4. Does flow narrowing cross calls, suspension points, or captured mutation?
5. Is an internal top type useful if no operations are permitted until narrowing?

## Compile time and attributes

1. Which ordinary functions are eligible for implicit constant evaluation, and
   which must be explicitly marked compile-time?
2. Which immutable aggregate types are valid UDA values?
3. Can a UDA handler emit only diagnostics/metadata, or may a later version use
   a hygienic typed declaration builder?
4. Which effect notation best describes compile-time-safe functions while
   remaining natural in Luau-like syntax?
5. Which explicit build inputs, if any, may compile-time code read without
   breaking reproducibility?
6. Is any runtime metadata retention needed in the first release, or should all
   initial UDA consumers be compile-time generated code?

## Runtime

1. Which initial memory manager best supports fast implementation and future VM
   reuse?
2. Are finalizers part of the language, runtime-only, or excluded?
3. Are strings UTF-8 byte sequences, Unicode scalar sequences, or a distinct
   string/bytes pair?
4. Does error handling use typed results, exceptions, panics, or a combination?
5. What concurrency guarantees exist for modules, objects, and the collector?
6. Which minimum private type metadata is required for GC, interface dispatch,
   checked casts, and stack traces?

## Toolchain and packaging

1. What source extension and command name should represent Pop Lang?
2. How are package identity and module identity encoded independently of paths?
3. Which artifact owns dependency locking and reproducible build metadata?
4. What stability promise applies to serialized MIR and future bytecode?

## Decision gate

A question must be resolved before implementation when it affects observable
semantics or would otherwise leak backend assumptions upward. Record the answer
as an ADR, including alternatives and consequences.
