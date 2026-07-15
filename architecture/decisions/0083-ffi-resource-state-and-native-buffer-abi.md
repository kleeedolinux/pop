# ADR 0083: FFI Resource State and Native Buffer ABI

- Status: accepted
- Date: 2026-07-15
- Supersedes: none
- Extends: ADR 0022, ADR 0039, ADR 0078, ADR 0081, and ADR 0082

## Context

ADR 0082 fixes the safe `Ffi.Buffer<T>` API and names
`Ffi.AllocationError` and `Ffi.NullPointerError`, but it does not assign their
stable identities or close the PLRI/native ABI used by the buffer operations.
Without that contract, one backend could confuse zero-length success with
allocation failure, treat a closed resource as a null pointer, expose a host
allocator pointer as object identity, or invent target layout independently.

The buffer is not a foreign ABI value. It is a Pop resource with stable
identity and mutable lifecycle whose owned bytes happen to live outside the
managed heap. Its implementation must therefore keep resource state distinct
from raw storage while remaining precise under collector relocation.

## Decision

### Stable expected-error identities

The reserved `Pop.Ffi` bootstrap type identities are completed as follows:

- built-in type 208 is `Ffi.NullPointerError`;
- built-in type 209 is `Ffi.AllocationError`.

Both are non-prelude, arity-zero, immutable singleton nominal expected-error
types. They carry no host error code, allocator string, pointer, or dynamic
payload. Compiler-known constructors produce only their exact singleton value.
They are ordinary typed `Result` error parameters and do not enable exceptions
or runtime reflection.

### Buffer representation and layout authority

`Ffi.Buffer<T>` is a managed resource reference, not a pointer-sized foreign
ABI scalar. It cannot appear directly in a foreign declaration. Its private
state contains one immutable canonical `FfiAbiLayoutId`, element count, owned
unmanaged allocation, and the closed/borrow lifecycle. Collector relocation
updates the managed reference normally; no backend or native library observes
the resource object's address or fields.

The compiler computes the canonical element size and alignment from the exact
selected target and accepted ABI layout. HIR and MIR carry the stable layout
identity and exact `T`; they never carry an LLVM type or recompute record
offsets in a backend. The runtime validates that every operation uses the
layout installed by `open`.

The runtime checks `length * elementSize` before allocation, uses alignment at
least that of `T`, and zero-initializes every byte. Element size and alignment
are nonzero powers accepted by the compiler. Invalid geometry or a mismatched
layout is a compiler/runtime invariant, never `AllocationError`.

### PLRI operations and native ABI 1.16

PLRI adds the backend-neutral operations `ffiBufferOpen`, `ffiBufferLength`,
`ffiBufferRead`, `ffiBufferWrite`, `ffiBufferBorrow`, `ffiBufferEndBorrow`, and
`ffiBufferClose`. Native ABI 1.16 exposes their exact C boundary:

```text
pop_rt_ffi_buffer_open(length, elementSize, alignment, layoutId, outBuffer) -> u8
pop_rt_ffi_buffer_length(buffer, layoutId, outLength) -> u8
pop_rt_ffi_buffer_read(buffer, layoutId, index, outElement, elementSize) -> u8
pop_rt_ffi_buffer_write(buffer, layoutId, index, element, elementSize) -> u8
pop_rt_ffi_buffer_borrow(buffer, layoutId, outPointer, outLength, outBorrow) -> u8
pop_rt_ffi_buffer_end_borrow(buffer, borrow) -> u8
pop_rt_ffi_buffer_close(buffer) -> u8
```

Integer values are unsigned 64-bit ABI values, `buffer` is a nonzero managed
reference, `layoutId` is a stable unsigned 64-bit artifact identity, pointer
arguments are native pointer-width addresses, and `borrow` is a nonzero opaque
generation token. All output storage is writable for exactly the stated value
and remains unchanged on failure.

`ffiBufferOpen` returns the closed status `0 = allocationFailure`,
`1 = success`, or `2 = invariantFailure`. Only status zero becomes the typed
`Ffi.AllocationError`; status two traps before managed code continues. Every
other operation returns `0 = invariantFailure` or `1 = success`. Bounds,
closed-state, forged-reference, layout, size, active-borrow, and borrow-token
failures are invariants and never masquerade as absent data or allocation
failure.

A successful zero-length open still returns a nonzero Buffer reference.
Borrowing it returns a null pointer, length zero, and a live nonzero borrow
token. A non-empty borrow returns a non-null pointer. `close` is idempotent on
the same Buffer object, but rejects closing while a borrow is live. Every
operation other than repeated close rejects a closed Buffer.

Read and write copy exactly one canonical element through backend-owned
temporary ABI storage. They check the complete index and byte range before
touching either side. The source, PLRI, and native ABI index is one-based:
exactly `1..length` is valid, including after optimization and across every
backend. Layout-record marshalling uses the ADR 0082 field plan; the runtime
only moves verified bytes and never interprets Pop object layout.

### Verified MIR behavior

Canonical MIR carries typed `ffiBufferOpen`, `ffiBufferLength`,
`ffiBufferRead`, `ffiBufferWrite`, `ffiBufferBorrow`, `ffiBufferEndBorrow`, and
`ffiBufferClose` operations. Open represents all three native statuses
explicitly before constructing `Result`; no zero sentinel becomes a Buffer.
Read, write, length, borrow, and close trap on failed status. Borrow operations
carry one lexical borrow-region identity, and the verifier proves dominance,
no escape or suspension, balanced end-borrow cleanup, and no close while live.

The MIR interpreter implements the same layout, zero initialization, bounds,
lifecycle, and status semantics without consulting LLVM or host C layout. ADR
0084 fixes the exact instruction operands, typed open result, target layout
catalog, and backend-private borrow-generation handling.

## Consequences

- Safe wrappers can own native arrays and structures without exposing managed
  object addresses.
- Allocation exhaustion remains an expected typed result while forged or
  inconsistent state fails closed.
- Buffer identity follows the collector; borrowed storage addresses never
  become object identity.
- Backends share one layout and lifecycle proof rather than implementing
  allocator conventions independently.

## Alternatives considered

### Represent Buffer as a raw pointer

Rejected because null would conflate allocation failure and zero length, close
would have no stable identity, and relocation/resource state would be lost.

### Keep closed handles forever in a native side table

Rejected because idempotent close would require unbounded tombstones and the
table would become a second unmanaged object system.

### Return one Boolean status for open

Rejected because allocation failure and compiler/runtime invariant failure
must not share an expected-error path.

## Required conformance tests

- exact bootstrap identities 208 and 209, dependency gating, and singleton
  typed values;
- zero and nonzero open, checked multiplication, alignment, zeroed bytes,
  allocation failure, and invariant-status separation;
- read/write first, last, zero, and greater-than-length indices for scalar,
  pointer, handle, and layout-record elements;
- idempotent close, use-after-close, close-during-borrow, forged reference,
  wrong layout, wrong element size, and unchanged outputs on failure;
- zero/nonzero borrow pointers, generation-checked end-borrow, no escape or
  suspension, and every-exit cleanup;
- forced collector relocation before each operation;
- MIR corruption, optimization preservation, MIR-interpreter behavior, and
  linked LLVM native fixtures using identical layout identities.

## Documents/components affected

Bootstrap FFI metadata, type checking, HIR, MIR, PLRI/runtime, collector,
native ABI, LLVM, MIR interpreter, artifact layout metadata, diagnostics,
conformance tests, and the implementation roadmap.
