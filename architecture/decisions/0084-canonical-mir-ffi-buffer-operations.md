# ADR 0084: Canonical MIR FFI Buffer Operations

- Status: accepted
- Date: 2026-07-15
- Supersedes: none
- Extends: ADR 0022, ADR 0081, ADR 0082, and ADR 0083

## Context

ADR 0083 requires seven typed buffer operations in canonical MIR, but does not
fix their exact operands and results. Native `ffiBufferOpen` has three statuses,
while a MIR instruction has one SSA result. Native `ffiBufferBorrow` returns a
pointer, immutable length, and opaque generation, while source `withPointer`
exposes only the pointer and length inside a lexical callback.

Leaving those shapes to backends would let LLVM hide a zero sentinel inside a
managed reference, let the interpreter skip generation validation, or make a
future VM invent a different layout-record path. The canonical operation forms
must preserve the accepted source and PLRI contracts without exposing runtime
tokens as Pop values.

## Decision

### Canonical layout catalog

Every MIR Bubble carries a deterministic FFI layout catalog selected for its
exact target. A catalog entry has one nonzero `FfiAbiLayoutId`, exact element
`TypeId`, byte size, power-of-two alignment, ABI value class, and, for
`Ffi.C.Layout` records, the ordered field marshalling plan accepted by ADR 0082.
The value class is closed to scalar integer/float, pointer, function pointer,
handle token, and layout record. Each record field names its source field
identity, field layout identity, byte offset, and recursively closed value
class. Padding is explicit zero-fill, never copied from managed object storage.

Catalog validation rejects duplicate identities, zero sizes or alignments,
misaligned/out-of-range fields, overlap, recursive by-value layouts, a type
mismatch, target mismatch, and any managed-reference-bearing value class. HIR
and MIR refer only to the validated catalog identity; LLVM types and host C
layout are not catalog authority.

### Open outcome

Canonical `ffiBufferOpen` carries:

- the one-time evaluated `Ffi.C.Size` length operand;
- exact element `TypeId`;
- nonzero layout identity, byte size, and alignment from the catalog;
- the exact `Result` definition plus success and failure case identities.

Its SSA result is exactly
`Result<Ffi.Buffer<T>, Ffi.AllocationError>`. Execution observes the closed
runtime outcome before constructing that value:

- allocation failure constructs the failure case with the exact singleton
  `Ffi.AllocationError` value;
- success requires a nonzero managed `Ffi.Buffer<T>` reference and constructs
  the success case;
- invariant failure, an unknown status, or a zero success reference traps
  before a result reaches managed code.

LLVM lowering therefore branches on all three ABI 1.16 statuses. The MIR
interpreter distinguishes `FfiBufferOpenFailure::Allocation` from its invariant
form before constructing the same nominal result. No backend treats zero as a
Buffer or turns allocation exhaustion into a trap.

### Length, read, write, and close

Canonical operations have these typed shapes:

```text
ffiBufferLength(buffer, layoutId) -> Ffi.C.Size
ffiBufferRead(buffer, oneBasedIndex, layoutId) -> T
ffiBufferWrite(buffer, oneBasedIndex, value, layoutId)
ffiBufferClose(buffer)
```

The verifier proves that `buffer` is exactly `Ffi.Buffer<T>`, the catalog entry
maps the same `T`, the index is `Ffi.C.Size`, and read/write result or value is
the exact `T`. Write and close are effect instructions without an SSA result.
Every operation retains the accepted invariant-trap and output-atomic behavior.

Scalar, pointer, function-pointer, and handle-token values use typed ABI
temporaries of their catalog width. Layout records marshal field-by-field
through catalog offsets and zero padding. Backends never copy a Pop record
object or consult a host compiler for layout.

### Lexical borrow generation

Canonical borrow operations are:

```text
ffiBufferBorrow(buffer, expectedLength, layoutId, borrowRegion) -> Ffi.OptionalPointer<T>
ffiBufferEndBorrow(buffer, borrowRegion)
```

`expectedLength` is the dominating result of `ffiBufferLength` for the same
buffer. Buffer length cannot change, so the native lowering receives pointer,
length, and generation from one ABI call, traps if the returned length differs,
and publishes only the optional pointer as SSA. The opaque generation is stored
in backend-private temporary state indexed by the typed `BorrowRegionId`; it is
not a Pop value and cannot be converted, returned, captured, or stored.

The MIR interpreter keeps the same region-to-generation association. End-borrow
uses the exact recorded generation and removes it only after runtime success.
Optimization may rename blocks and SSA values but cannot merge, duplicate,
erase, or synthesize borrow-region identities.

The verifier proves one dominating length, one successful borrow, one balanced
end on every normal and unwind exit, non-escape of the pointer, no suspension,
and no close, move, or second borrow while the region is live. Cleanup is
registered before callback execution. Corrupt MIR fails verification rather
than reaching a backend.

## Consequences

- Open preserves typed allocation failure without weakening invariant traps.
- Native generation tokens remain explicit in backend execution state without
  becoming forgeable source or MIR scalar values.
- The interpreter, LLVM, and future VM share one one-based, layout-checked,
  relocation-safe contract.
- Layout records have one target-selected field plan instead of backend or host
  layout guesses.

## Alternatives considered

### Return a raw Buffer and use zero for allocation failure

Rejected because zero is not a managed resource reference and would conflate an
expected typed failure with an invariant.

### Return pointer, length, and generation as a Pop tuple

Rejected because it would make the generation observable and storable, and a
managed tuple would violate the lexical non-escape proof.

### Recompute layout independently in each backend

Rejected because HIR/MIR must remain the cross-backend semantic contract and
host layout is not target authority.

## Required conformance tests

- catalog identity, target, size, alignment, value-class, field-offset, padding,
  overlap, recursion, and managed-reference rejection;
- open allocation/success/invariant branches, exact `Result` cases, singleton
  error identity, zero-success rejection, and unknown-status rejection;
- verifier rejection for wrong Buffer element, layout, index, value, result,
  missing result cases, and effect/value form;
- scalar, pointer, function-pointer, handle, and nested layout-record
  read/write differential tests at first, last, zero, and past-end indices;
- borrow dominance, length equality, generation matching, non-escape,
  no-suspend, no-close, no-double-borrow, and every-exit cleanup;
- optimization preservation plus MIR-interpreter, LLVM native, and future VM
  differential behavior.

## Documents/components affected

Target layout metadata, HIR, MIR construction and verification, optimization,
MIR interpreter, LLVM lowering, native ABI calls, diagnostics, and conformance
tests.
