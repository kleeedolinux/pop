# ADR 0082: FFI ABI Storage and Lexical Borrows

- Status: accepted
- Date: 2026-07-15
- Supersedes: the unrestricted generic managed-value pin interpretation in
  ADR 0081
- Extends: ADR 0022, ADR 0039, ADR 0052, ADR 0078, and ADR 0081

## Context

ADR 0081 requires raw foreign pointers to refer only to unmanaged storage or a
compiler-verified lexical pin. It names `Ffi.withPin(value, ...)`, but does not
close the representation contract for `value`. An arbitrary Pop record, array,
class, closure, or string does not have C-compatible storage. Returning its
private object address would expose collector and backend layout, make pointer
arithmetic depend on implementation details, and create an untracked managed
pointer escape hatch.

Productive native code still needs three short paths:

- immutable bytes borrowed for one synchronous call;
- owned contiguous ABI storage for input/output arrays and structures;
- generation-checked managed handles for native work that outlives one call.

These paths need exact nullability, close, bounds, movement, and marshalling
semantics before compiler or runtime intrinsics can be stabilized.

## Decision

### Pointer vocabulary

`Pop.Ffi` adds the non-prelude nominal types `Ffi.ReadOnlyPointer<T>` and
`Ffi.OptionalReadOnlyPointer<T>` beside ADR 0081's `Ffi.Pointer<T>` and
`Ffi.OptionalPointer<T>`. All four have the target pointer ABI, but their Pop
types remain distinct:

- `ReadOnlyPointer<T>` permits typed reads and cannot be used by a Pop write;
- `Pointer<T>` permits typed reads and writes only under `Ffi.Unsafe`;
- each `Optional` form is the only corresponding ABI type that can contain a
  null address;
- no pointer implicitly converts to an integer, `nil`, a managed reference, or
  a pointer with a different element type.

The exact safe nullability operations are:

```luau
public function Ffi.OptionalPointer.none<T>(): Ffi.OptionalPointer<T>
public function Ffi.OptionalPointer.fromPointer<T>(pointer: Ffi.Pointer<T>): Ffi.OptionalPointer<T>
public function Ffi.OptionalPointer.isPresent<T>(pointer: Ffi.OptionalPointer<T>): Boolean
public function Ffi.OptionalPointer.require<T>(pointer: Ffi.OptionalPointer<T>): Result<Ffi.Pointer<T>, Ffi.NullPointerError>

public function Ffi.OptionalReadOnlyPointer.none<T>(): Ffi.OptionalReadOnlyPointer<T>
public function Ffi.OptionalReadOnlyPointer.fromPointer<T>(pointer: Ffi.ReadOnlyPointer<T>): Ffi.OptionalReadOnlyPointer<T>
public function Ffi.OptionalReadOnlyPointer.isPresent<T>(pointer: Ffi.OptionalReadOnlyPointer<T>): Boolean
public function Ffi.OptionalReadOnlyPointer.require<T>(pointer: Ffi.OptionalReadOnlyPointer<T>): Result<Ffi.ReadOnlyPointer<T>, Ffi.NullPointerError>

public function Ffi.Pointer.readOnly<T>(pointer: Ffi.Pointer<T>): Ffi.ReadOnlyPointer<T>
```

`NullPointerError` is one exact nominal expected error. Requiring an absent
pointer returns that error; it does not panic or manufacture an address.

The first `Ffi.Unsafe` memory operations are closed compiler-known functions:

```luau
public function Ffi.Unsafe.load<T>(pointer: Ffi.ReadOnlyPointer<T>): T
public function Ffi.Unsafe.store<T>(pointer: Ffi.Pointer<T>, value: T)
public function Ffi.Unsafe.advance<T>(pointer: Ffi.Pointer<T>, elements: Ffi.C.PointerDifference): Ffi.Pointer<T>
public function Ffi.Unsafe.advanceReadOnly<T>(pointer: Ffi.ReadOnlyPointer<T>, elements: Ffi.C.PointerDifference): Ffi.ReadOnlyPointer<T>
public function Ffi.Unsafe.copy<T>(source: Ffi.ReadOnlyPointer<T>, destination: Ffi.Pointer<T>, count: Ffi.C.Size)
public function Ffi.Unsafe.address<T>(pointer: Ffi.ReadOnlyPointer<T>): Ffi.C.Size
public function Ffi.Unsafe.pointerFromAddress<T>(address: Ffi.C.Size): Ffi.OptionalPointer<T>
```

`T` must have one accepted ABI storage layout. Arithmetic checks address and
element-size overflow. Load/store check target alignment and lexical
provenance/bounds when the pointer carries them. A foreign-origin raw pointer
has no invented bounds; its caller remains inside the explicit unsafe
contract. `copy` has `memmove` overlap semantics and checks the complete byte
count before performing an operation. These functions never inspect or create
a Pop managed-object reference.

### Owned unmanaged buffers

`Ffi.Buffer<T>` is a small nominal resource class with stable identity and an
explicit mutable lifecycle. It owns contiguous target-ABI storage outside the
managed heap. `T` must be an accepted ABI scalar, pointer/function-pointer
type, handle token, or verified `Ffi.C.Layout` record. Managed strings,
classes, arrays, tables, closures, interfaces, and unannotated records are
rejected as elements.

The exact first API is:

```luau
public function Ffi.Buffer.open<T>(length: Ffi.C.Size): Result<Ffi.Buffer<T>, Ffi.AllocationError>
public function Ffi.Buffer.length<T>(buffer: Ffi.Buffer<T>): Ffi.C.Size
public function Ffi.Buffer.read<T>(buffer: Ffi.Buffer<T>, index: Ffi.C.Size): T
public function Ffi.Buffer.write<T>(buffer: Ffi.Buffer<T>, index: Ffi.C.Size, value: T)
public function Ffi.Buffer.withPointer<T, R>(buffer: Ffi.Buffer<T>, body: function(pointer: Ffi.OptionalPointer<T>, length: Ffi.C.Size): R): R
public function Ffi.Buffer.close<T>(buffer: Ffi.Buffer<T>)
```

`open` checks `length * sizeOf(T)` and returns `AllocationError` without a
partial allocation. Storage is aligned for `T` and zero-initialized. `read`
and `write` are bounds-checked. A zero-length buffer supplies an absent pointer
and length zero; a non-empty live buffer supplies a present pointer.

`close` is idempotent so it is safe in `defer`. Every other operation on a
closed buffer is an invariant panic. Closing, moving, or resizing a buffer
while its pointer borrow is active is rejected statically. A buffer has no
finalizer and failure to close is diagnosed by the resource-lifetime checker;
the collector does not guess ownership.

### Lexically pinned immutable bytes

The stable pin surface is deliberately not generic over arbitrary managed
values. Its first and only overload borrows the contiguous payload of immutable
`Bytes`:

```luau
public function Ffi.withPin<R>(bytes: Bytes, body: function(pointer: Ffi.OptionalReadOnlyPointer<Byte>, length: Ffi.C.Size): R): R
```

The runtime pins the exact `Bytes` owner and returns its payload address through
a reviewed PLRI operation; native code never receives the Pop object-header
address. Empty bytes supply an absent pointer and length zero. The borrow is
synchronous and non-escaping. The checker rejects returning, storing,
capturing, address-converting, or passing the pointer to an unverified ordinary
function, and rejects suspension anywhere in the borrow. Passing it to an
exact foreign declaration is permitted because stable foreign declarations
cannot claim retention.

Pin release is registered as lexical cleanup before the body begins and runs
exactly once on normal return, expected failure, panic unwind, and cancellation.
HIR and MIR carry a typed borrow-region identity plus explicit `Pin`/`Unpin`;
the verifier proves dominance, non-escape, no suspension, and every-exit
cleanup. The MIR interpreter and every backend consume the same proof.

Future managed containers may add separate overloads only after their
contiguous storage, read/write aliasing, and view-lifetime contracts are
accepted. `Array<T>`, a class instance, a closure, `String`, and a normal record
do not become pinnable by structural coincidence.

### Generation-checked managed handles

`Ffi.Handle<T>` is the only stable long-lived native token for one managed
value. `T` must be a managed reference representation; scalar values use
ordinary ABI values or copied buffer storage. The exact first API is:

```luau
public function Ffi.Handle.open<T>(value: T): Ffi.Handle<T>
public function Ffi.Handle.get<T>(handle: Ffi.Handle<T>): T
public function Ffi.Handle.close<T>(handle: Ffi.Handle<T>)
```

`open` registers one runtime strong root and returns a nonzero token containing
a slot identity and generation. `get` resolves that exact slot to its current
post-relocation value. `close` consumes the generation and releases the root.
A forged, zero, stale, wrong-generation, or already-closed token is an
invariant panic with no managed value returned. Native code may preserve and
return the token but cannot dereference it or reinterpret it as a pointer.

Native ABI 1.15 adds
`pop_rt_resolve_root(u64 handle) -> u64 managedReference`. Zero reports a
failed invariant at this narrow C boundary; valid managed references and root
handles are nonzero. The generated `Ffi.Handle.get` adapter traps on zero
before managed code resumes. Handle creation and close continue to use the
existing `pop_rt_retain_root` and `pop_rt_release_root` entries. PLRI names the
backend-neutral operation `resolveRoot`; MIR carries the exact static managed
result type and never treats the returned token as a pointer.

### Fixed-layout record marshalling

An `Ffi.C.Layout` record remains an ordinary Pop record, not an exposed object
layout. At a foreign by-value boundary, the compiler marshals its ordered
fields into target ABI storage. A returned value is unmarshalled into a normal
Pop record. `Ffi.Buffer<T>.read` and `write` use the same mapping. No pointer to
the normal record is produced.

For each selected target and ABI, the compiler computes size, alignment, field
offsets, and one lowercase SHA-256 fingerprint over canonical UTF-8 JSON with
this logical schema:

```json
{"schemaVersion":1,"target":"x86_64-unknown-linux-gnu","abi":"C","size":8,"alignment":4,"fields":[{"name":"left","abiType":"Int32","offset":0,"size":4,"alignment":4},{"name":"right","abiType":"Int32","offset":4,"size":4,"alignment":4}]}
```

Object keys use the shown order, decimal integers have no leading zero, strings
use canonical JSON escaping, and there is no trailing newline. A nested layout
uses `layout:<lowercase fingerprint>` as `abiType`; pointer types use their
complete qualified pointer kind and recursively canonical element ABI type.
The fingerprint is independent of session-local IDs and backend data
structures.

The `.poplib` target manifest maps the stable declaration identity to these
facts and the fingerprint. Generated `native-bindings.json` supplies the same
facts from the approved parser. A missing or unequal field, order, size,
alignment, offset, target, ABI, or fingerprint fails before HIR/MIR reaches a
backend.

Marshalling allocation and cleanup effects remain explicit. HIR/MIR use a
backend-neutral foreign-layout identity and field plan; they contain no LLVM
types, offsets reconstructed by a backend, or C-specific source strings.

## Consequences

- Common byte calls use one `Ffi.withPin` and do not copy.
- General arrays and structures use an explicitly owned `Ffi.Buffer<T>` rather
  than exposing collector storage.
- Native work can retain a typed `Ffi.Handle<T>` without retaining a raw
  managed address.
- Empty native spans preserve exact nullability without turning `nil` into a
  generic pointer.
- Fixed-layout records remain normal Pop data and are marshalled rather than
  becoming backend object-layout aliases.
- The stable surface adds read-only pointer types and resource checking, but
  prevents a much larger unsafe object-layout and lifetime contract.

## Alternatives considered

### Pin every `T` and return the object address

Rejected because most Pop values have no C-compatible representation and the
address would expose private collector/backend layout.

### Treat every native buffer as `Array<Byte>`

Rejected because Pop arrays have typed managed semantics, not a promise of
byte-packed target ABI storage, alignment, or native retention.

### Make pointer nullability use `nil`

Rejected because generic optional values and ABI null pointers have different
representation and safety contracts.

### Rely on finalizers for buffers and handles

Rejected because cleanup timing would become nondeterministic and would add
the finalization/resurrection obligations excluded by the GC architecture.

## Required conformance tests

- exact bootstrap identities and explicit-dependency gating for the added
  pointer/buffer APIs;
- immutable-to-mutable pointer, implicit `nil`, integer conversion, wrong
  element, misalignment, overflow, and unsupported managed-element negatives;
- zero/nonzero buffer allocation, bounds, alignment, idempotent close,
  use-after-close, allocation failure, and every-exit cleanup;
- `Bytes` pin success plus return/store/capture/ordinary-call/address/suspension
  escape negatives;
- MIR dominance, region, pin/unpin, cleanup, optimization-preservation, and
  verifier-corruption tests;
- forced relocation while bytes are pinned and after unpin, with no object
  header exposed;
- handle relocation, forged/zero/stale/wrong-generation/double-close tests;
- exact layout scalar/nested/pointer facts, canonical fingerprints, generated
  metadata mismatch, by-value call/return, and buffer marshalling tests;
- MIR-interpreter capability behavior and LLVM native fixtures using the same
  semantic plans;
- architecture regressions forbidding arbitrary managed-value pinning, raw
  object addresses, implicit finalizers, and untracked pointer escape.

## Documents/components affected

Type system, HIR, MIR, PLRI/runtime, collector pin/root tables, LLVM, MIR
interpreter, `Pop.Ffi`, `Bytes`, `.poplib`, generated binding metadata,
diagnostics, conformance policy, and implementation roadmap.
