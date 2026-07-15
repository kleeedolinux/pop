# ADR 0087: Scoped FFI Borrow Bodies and Bytes Pin ABI

- Status: accepted
- Date: 2026-07-15
- Supersedes: none
- Extends: ADR 0022, ADR 0052, ADR 0081, ADR 0082, ADR 0083, ADR 0084,
  and ADR 0086

## Context

ADR 0082 accepts `Ffi.Buffer.withPointer` and the immutable-`Bytes`
`Ffi.withPin` overload. ADR 0084 fixes the buffer borrow operations and their
region identity, but neither decision fixes how the body function is admitted,
how its non-escape proof survives into MIR, or how a runtime returns the
`Bytes` payload address instead of the managed object address.

Treating the body as an unrestricted function value would be unsound. Its
ordinary pointer parameter has no source lifetime parameter, so a separately
compiled function could store, capture, return, or publish it. Letting each
backend infer that a closure is harmless would also make LLVM, the MIR
interpreter, and a future VM enforce different borrow rules. Finally, the
existing private `pin` operation returns only a pin token. It cannot expose the
exact immutable payload address and length without an additional reviewed
runtime contract.

## Decision

### Source body is one immediate closure

The body argument of `Ffi.Buffer.withPointer` and `Ffi.withPin` must be a
non-async closure expression written directly at that call site. A local,
parameter, field, returned function value, or separately declared function is
rejected even when its static function signature is otherwise equal. This is a
proof boundary, not overload ranking or runtime inspection.

The closure has exactly the accepted two parameters and one exact result pack:

```luau
Ffi.Buffer.withPointer<T, R>(
    buffer,
    function(pointer: Ffi.OptionalPointer<T>, length: Ffi.C.Size): R
        -- synchronous scoped body
    end,
)

Ffi.withPin<R>(
    bytes,
    function(pointer: Ffi.OptionalReadOnlyPointer<Byte>, length: Ffi.C.Size): R
        -- synchronous scoped body
    end,
)
```

Ordinary lexical captures remain allowed. The closure itself never becomes a
first-class result or stored value; HIR records it as the body of one scoped
borrow operation. This restriction can be widened only after Pop Lang has an
accepted cross-Bubble lifetime contract capable of expressing the same proof.

### Closed borrow-provenance analysis

The checker assigns one internal `BorrowRegionId` to the call and marks the
pointer parameter as borrowed from that region. The region is compiler proof
metadata and never becomes a source type argument, runtime value, or reflected
fact.

Borrow provenance follows only closed typed operations. Presence testing and
checked extraction preserve the region. A derived required pointer may be
passed to an exact foreign declaration because stable foreign declarations
cannot claim retention. It may not be:

- returned or included in the scoped body's result;
- assigned to a field, array, table, capture cell, outer local, or managed
  aggregate;
- captured by another closure;
- passed to an ordinary, referenced, interface, method, or indirect call;
- converted to an address or another element/mutability type;
- retained across suspension, task creation, callback registration, or another
  lexical borrow.

The first implementation also rejects `Ffi.Unsafe` arithmetic, load, store,
and copy on a scoped pointer. Those operations require a separate bounds proof
that preserves the associated length. Buffer element access remains available
through checked `Ffi.Buffer.read`/`write`, while the borrowed pointer remains
productive for exact foreign calls.

The complete scoped body is synchronous. Any direct or transitive `Suspends`
effect is rejected. Its returned values must be independent of the borrowed
pointer by the same dataflow proof; returning an unrelated statically obtained
foreign pointer is not rejected merely because its type is a pointer.

### Canonical HIR and MIR region call

HIR owns distinct `FfiBufferWithPointer` and `FfiBytesWithPin` expressions.
Each retains the resolved inline closure, exact element/result types, layout
identity where applicable, and one `BorrowRegionId`. They are not ordinary
calls selected again from source spelling.

Canonical buffer MIR is:

```text
length = ffiBufferLength(buffer, layoutId)
pointer = ffiBufferBorrow(buffer, length, layoutId, borrowRegion)
results = callScopedBorrow(borrowRegion, nestedFunction, captures,
                           pointer, length)
ffiBufferEndBorrow(buffer, borrowRegion)
```

Canonical immutable-byte MIR is:

```text
pointer = ffiBytesBorrow(bytes, borrowRegion)
length = ffiBytesBorrowLength(bytes, borrowRegion)
results = callScopedBorrow(borrowRegion, nestedFunction, captures,
                           pointer, length)
ffiBytesEndBorrow(bytes, borrowRegion)
```

`callScopedBorrow` names the statically resolved nested closure identity and
its exact captures, parameters, results, and effects. It is not an indirect
runtime lookup and does not allocate a general closure environment. Backends
may use their ordinary direct nested-call convention after consuming the same
verified region proof.

The verifier inspects both the caller region and the named nested body. It
proves start dominance, exact pointer/length arguments, permitted provenance
uses, no suspension or nested borrow, one matching cleanup on every normal,
expected-failure, panic-unwind, and cancellation exit, and no result alias.
The optimizer preserves the region and nested-function identity. Missing or
corrupt proof is invalid MIR rather than a backend fallback.

Cleanup is registered immediately after a successful borrow and before the
body executes. `callScopedBorrow` carries the ordinary explicit unwind action;
normal and unwind paths execute the same region-specific end operation exactly
once.

### Immutable Bytes payload borrow

PLRI adds one closed result value:

```text
FfiBytesBorrow {
    token: FfiBytesBorrowId,
    address: Optional<ForeignAddress>,
    length: Ffi.C.Size,
}
```

and two operations:

```text
ffiBytesBorrow(bytes: ManagedReference) -> FfiBytesBorrow
ffiBytesEndBorrow(bytes: ManagedReference, token: FfiBytesBorrowId)
```

The runtime verifies the exact trusted immutable `Bytes` representation, pins
its owner, and returns only the first payload byte plus the immutable byte
length. Empty bytes return no address and length zero. Non-empty bytes return a
nonzero address. The object header, side metadata, allocator padding, and any
mutable storage are never exposed. A stale, forged, wrong-owner, duplicate, or
already-ended token is an invariant failure.

Native ABI 1.17 adds:

```text
pop_rt_ffi_bytes_borrow(
    u64 bytes,
    u64* outAddress,
    u64* outLength,
) -> u64 borrowToken

pop_rt_ffi_bytes_end_borrow(
    u64 bytes,
    u64 borrowToken,
) -> u8 status
```

Borrow returns zero on invariant failure and leaves both outputs unchanged.
On success it returns a nonzero token and writes the exact null-or-nonzero
address and length pair. End returns `1` only after consuming the exact live
token; `0` is an invariant failure. Native ABI 1.17 does not reinterpret the
older private `pop_rt_pin`/`pop_rt_unpin` entries.

The backend stores the token and immutable length in private state keyed by
`BorrowRegionId`. Only the optional pointer and length become Pop SSA values.
PLRI and native ABI operations are capability boundaries; the compiler never
reconstructs a `Bytes` payload offset from object layout.

## Consequences

- The productive call site stays short while its lifetime proof remains
  explicit and backend-neutral.
- Separately compiled ordinary functions cannot impersonate a non-retaining
  borrow body.
- Buffer and immutable-byte borrows share one scoped-call verifier contract.
- The collector/runtime owns `Bytes` payload knowledge; LLVM and MIR never
  expose or calculate a managed object address.
- Native ABI 1.17 is additive and preserves older private pin operations.

## Alternatives considered

### Accept any matching function value

Rejected because the signature contains no lifetime capable of preventing a
separately compiled body from retaining the pointer.

### Inline the closure by rewriting its source locals into the caller

Rejected as the canonical contract because it would make cleanup and return
rewriting an implementation-dependent HIR transformation. A named nested body
with one verified region call is smaller and stable across backends.

### Return a managed tuple containing pointer, length, and token

Rejected because the token would become observable and storable, and borrowing
immutable bytes must not allocate a managed tuple.

### Reuse generic pin and calculate the payload offset in LLVM

Rejected because object layout is not an FFI contract and a future VM or
collector may use a different representation.

## Required conformance tests

- direct inline closure success for zero and nonzero buffer/byte payloads;
- named/local/parameter body, async body, wrong arity/type/result, nested
  borrow, and transitive suspension rejection;
- return, field/collection/capture/aggregate store, nested capture, ordinary
  call, address conversion, unsafe operation, callback, and task escape
  negatives;
- exact foreign-call success with required and optional scoped pointers;
- HIR/MIR region identity, direct nested-body identity, dominance, taint,
  result independence, balanced normal/failure/unwind/cancellation cleanup,
  and optimizer-corruption tests;
- PLRI and native ABI zero/nonzero payload, unchanged failure outputs,
  wrong-owner, stale, forged, duplicate-end, and forced-relocation tests;
- MIR-interpreter, LLVM, and future VM differential execution of the same
  verified region plan;
- architecture regressions forbidding unrestricted function bodies, generic
  managed-value pins, object-header offsets, dynamic lifetime checks, and
  backend-private borrow semantics.

## Documents/components affected

Type checking, capture/effect analysis, HIR, MIR, cleanup lowering,
optimization, PLRI, native ABI 1.17, immutable `Bytes` runtime storage, the
collector, MIR interpreter, LLVM, diagnostics, and FFI conformance tests.
