# ADR 0092: Typed FFI Callbacks and Native Transition ABI

- Status: accepted
- Date: 2026-07-16
- Supersedes: none
- Extends: ADR 0003, ADR 0008, ADR 0022, ADR 0039, ADR 0077,
  ADR 0080, ADR 0081, ADR 0082, and ADR 0087
- Generator schema fixed by: ADR 0094

## Context

ADR 0081 accepts `Ffi.Function<TSignature>`, call-scoped
`Ffi.withCallback`, and explicitly owned callbacks, but leaves several choices
open. It does not define how a capturing Pop closure reaches a C callback,
which source signature represents the native function pointer, how a callback
enters managed execution, what closes a registration, or how thread,
concurrency, reentrancy, and panic policies are enforced.

Those choices cannot be backend conveniences. Passing a Pop closure address as
`void *`, resolving a callback by a runtime string, allowing a scoped callback
to escape, or letting a panic unwind through C would violate static typing,
precise collection, and backend equivalence. A useful first release still needs
the common C shape consisting of one typed function pointer and one opaque
userdata argument.

## Decision

### Exact source signatures

`Ffi.CallbackContext` is a non-generic opaque foreign-address type supplied by
`Pop.Ffi`. It has the target pointer ABI but no public constructor, integer
conversion, dereference, address operation, equality operation, or managed
identity. It may appear only:

- exactly once in a callback function signature;
- as the matching userdata parameter of a foreign declaration; or
- as the context value supplied beside an exact `Ffi.Function<TSignature>`.

The first stable callback shape uses these compiler-known APIs:

```luau
public function Ffi.withCallback<TSignature, R>(
    callback: TSignature,
    body: function(
        callbackFunction: Ffi.Function<TSignature>,
        context: Ffi.CallbackContext
    ): R
): R

public function Ffi.Callback.open<TSignature>(
    callback: TSignature,
    thread: Ffi.CallbackThread
): Result<Ffi.RegisteredCallback<TSignature>, Ffi.CallbackOpenError>

public function Ffi.Callback.withPair<TSignature, R>(
    callback: Ffi.RegisteredCallback<TSignature>,
    body: function(
        callbackFunction: Ffi.Function<TSignature>,
        context: Ffi.CallbackContext
    ): R
): Result<R, Ffi.CallbackClosedError>

public function Ffi.Callback.close<TSignature>(
    callback: Ffi.RegisteredCallback<TSignature>
): Result<(), Ffi.CallbackInUseError>
```

`TSignature` must be one non-async Pop function type whose parameters and
result have exact ADR 0081 ABI storage. It contains exactly one
`Ffi.CallbackContext` parameter. The managed callback receives all native
arguments, including the opaque context, and normally names that parameter `_`.
It has no variadic pack, receiver, managed parameter/result, suspension effect,
or native-unwind ABI. The compiler selects the target C ABI unless generated
callback metadata selects the closed `System` ABI. `CUnwind` is never a
callback ABI.

`Ffi.withCallback` requires both arguments to be immediate non-async closures.
The callback may capture statically typed managed values. Its scope body
receives one inseparable function/context pair; the checker permits that pair
only as the exact arguments of a foreign declaration whose trusted callback
parameter metadata says `CallScoped`. Returning, storing, separately passing,
capturing, address-converting, comparing, or retaining either member is an
error. The scope cannot suspend. Its callback registration is closed on every
normal, expected-failure, panic, unwind, and cancellation exit.

The `thread` argument of `Ffi.Callback.open` must be the direct spelling
`Ffi.CallbackThread.AttachedThread` in the first stable source contract. It is
a required compile-time constant carried in HIR, MIR, and metadata; the
`CallingThread` case, a local, parameter, conditional, integer, or runtime enum
value is rejected for owned registration.

`Ffi.Callback.open` creates an explicitly owned
`Ffi.RegisteredCallback<TSignature>` stable-identity lifecycle object. Aliases
refer to the same open/active/closed state; Pop Lang does not require an affine
or move-only type to enforce callback lifecycle. The object is neither a
pointer nor a managed function value and never exposes its function/context
pair as independently storable values.

ADR 0094 fixes the first stable lifetime/thread mapping so the fact survives
aliases without dependent or phantom types: call-scoped pairs are always
`CallingThread`, while owned `RegisteredCallback<TSignature>` pairs are always
`AttachedThread`. The registered type does not retain an arbitrary enum-value
refinement, so registered `CallingThread` is a later architecture gap and only
a runtime-internal capability.

`Ffi.Callback.withPair` requires one immediate non-async body and returns
`Ffi.CallbackClosedError` without running it when the shared lifecycle is
closed. Inside that lexical body, the inseparable pair may be passed only to a
generated foreign parameter pair whose metadata says `Registered`. Native code
may retain that registered pair according to the declaration metadata, but Pop
source cannot return, store, capture, separately pass, compare, or mix either
member. Native unregistration must complete before `close`. Close is
idempotent after a completed close, fails with `Ffi.CallbackInUseError` while an
entry is active, and invalidates the context generation before releasing the
root. Invocation after close is a native-contract violation that the generated
thunk contains at its panic boundary.

Source idempotency belongs to the registered object's shared lifecycle state: a
repeated source `close` observes `Closed` and performs no runtime operation.
The native close entry consumes and removes the registration on its first
success; duplicate native close fails instead of retaining an unbounded
tombstone registry.

`Ffi.CallbackThread` is a closed enum with `CallingThread` and
`AttachedThread`. The first stable source mapping is fixed by lifetime:
call-scoped pairs use `CallingThread`, while owned registered pairs use
`AttachedThread`. Calling-thread entry requires the same thread while it is
executing an active foreign transition which received the exact pair.
Attached-thread entry records the creating logical scheduler and permits entry
from an otherwise unattached native thread through a balanced managed-thread
attachment. A thread already attached to a different scheduler is rejected.
There is no ambient scheduler lookup or user-supplied numeric scheduler
identity.

The first stable callbacks are serialized and non-reentrant. The runtime
rejects overlapping or nested entry of the same registration before managed
code runs. These facts are explicit in generated metadata even though the
first public API does not offer weaker alternatives. Concurrent and reentrant
callbacks remain architecture gaps until Pop Lang has a static shared-capture
contract that can prove them sound.

A foreign declaration with callback parameter metadata is always blocking and
cannot carry `Ffi.Nonblocking`. Callback managed execution may allocate and
reach collecting safe points, so entry is permitted only from a blocking
foreign transition whose live roots were promoted to runtime-owned handles.
The checker and MIR verifier reject the callback-pair/`Ffi.Nonblocking`
combination, and the runtime rejects callback entry from `BoundedForeign`
without changing the registration or output.

### Foreign declaration metadata

Generated low-level declarations describe each function/context parameter pair
with one exact trusted attachment whose canonical facts are:

```text
callback parameter index
context parameter index
lifetime = CallScoped | Registered
ABI = C | System
signature layout fingerprint
thread = CallingThread | AttachedThread
concurrency = Serialized
reentrancy = Forbidden
panic = AbortProcess
```

For stable source attachments, `CallScoped` pairs require `CallingThread` and
`Registered` pairs require `AttachedThread`; crossed combinations fail before
HIR. The wider runtime enum does not authorize an unrepresented source policy.

Indices are compile-time parameter identities, not runtime lookup. The full
callback signature descriptor and SHA-256 layout fingerprint enter source
metadata, HIR, MIR, `.poplib`, and `native-bindings.popc`. Compact execution
keys follow ADR 0086 and cannot replace the full collision check. The checker
requires the function and context arguments to be the matching pair from one
registration and rejects lifetime or policy mismatches.

ADR 0094 supplies the closed generator encoding: schema 1 remains callback-free
and schema 2 carries exact zero-based indices, inline typed signature, full
fingerprint, lifetime, ABI, thread, serialized/non-reentrant policy, and abort
policy. The trusted attachment is descriptor-only and must match ordinary
generated source before it enters `ForeignFunctionDeclaration`.

The first stable panic policy is `AbortProcess`. Generated thunks balance every
runtime transition and then invoke the runtime's declared panic boundary. A
Pop panic, runtime invariant failure, invalid context, stale generation, wrong
thread, overlapping entry, or reentrant entry never unwinds through foreign
frames and never becomes a fabricated callback result. Expected failure is an
ordinary exact ABI result chosen by the callback body. Typed return-constant
or error-slot policies require a later ADR because their valid representation
depends on the exact result layout.

### Callback environment and provenance

The compiler lowers callback captures into one exact typed managed environment.
The runtime retains that environment through a generation-checked strong root.
It never exposes the environment or another Pop object address to native code.
Captureless callbacks use the same registration path with an empty environment
so lifetime and close behavior do not diverge.

The native-visible context is a runtime-owned opaque nonzero address token. The
runtime treats its bits solely as a lookup key and never dereferences them.
Each registration also has a private nonzero generation and one compile-time
`FfiCallbackSiteId`. A generated thunk passes its embedded site identity when
entering. Wrong-site, zero, forged, stale, and closed contexts fail before a
managed reference is returned. Opening a registration publishes only this
context and the typed environment/site identity; it does not choose a physical
callback ABI. Each verified pair scope selects a backend-emitted fixed typed
thunk address from its trusted generated contract. Multiple physical ABI thunks
may therefore share one registered context, but no runtime branch, symbol name,
or indirect unknown signature selects between them. ABI 1.18 callback support
is target-gated to ABIs whose pointer width
is exactly 64 bits, so converting the complete nonzero context token to and
from the userdata pointer cannot truncate or collide. A 16-bit, 32-bit, or
wider pointer target rejects callback lowering before link; it never masks or
compresses the token. Supporting another pointer width requires a distinct
closed context encoding and ABI revision.

### HIR and canonical MIR

HIR owns distinct `FfiWithCallback`, `FfiCallbackOpen`,
`FfiCallbackWithPair`, and `FfiCallbackClose` expressions. Open expressions
carry the exact source callback signature, generated callback body identity,
captures, thread/lifetime policy, and source region or owned resource identity.
Each pair expression additionally carries the one exact generated callback
contract selected from its lexically proven foreign uses: ABI, full signature
fingerprint, layouts, lifetime, thread, concurrency, reentrancy, and panic
policy. An unused pair or pair body with incompatible contracts is rejected
before HIR. HIR does not contain a native function address.

Canonical MIR owns backend-neutral operations:

```text
ffiCallbackOpenScoped{site, sourceSignature, callback, captures, region}
ffiCallbackOpenOwned{site, sourceSignature, callback, captures, thread}
callScopedCallback{registration, region, bindingSignature, body}
callRegisteredCallbackPair{registration, site, bindingSignature, region, body}
ffiCallbackCloseScoped{registration, region}
ffiCallbackCloseOwned{registration}
```

The callback body is one named nested MIR function with the exact source
parameters and result plus typed direct captures. `bindingSignature` is a
backend-neutral closed value containing callback ABI, full fixed fingerprint,
and parameter/result `FfiAbiLayoutId` facts. `FfiCallbackSiteId` and
`BorrowRegionId`-style scoped provenance are compiler identities, not runtime
strings. The verifier proves exact signature/layout equality, one context
parameter, function/context pair provenance, region dominance, non-escape,
non-suspension, every-exit scoped close, shared owned-resource lifecycle state,
lexical registered-pair use, and absence of
backend-specific calling conventions. It rejects a callback body with
`Suspends`, a pair passed to ordinary calls, mismatched pairs, close while an
entry may be active, and registered-pair use outside `withPair`. Aliases observe
the same closed state rather than receiving an invented move operation.

The MIR interpreter uses an exact typed test adapter for callback registration
and invocation. Without one it reports the unavailable native capability. The
experimental C backend rejects all callback operations. LLVM emits one fixed
typed thunk per callback-site/binding-signature pair from verified MIR. A
registered open is ABI-neutral; only the lexical pair operation materializes
the compile-time-selected C or System thunk address beside the context.

### PLRI and native ABI 1.18

PLRI adds opaque nonzero `FfiCallbackRegistrationId`,
`FfiCallbackTransitionId`, and `FfiCallbackSiteId` identities plus the closed
`FfiCallbackLifetime` and `FfiCallbackThread` enums. Its semantic operations
are:

```text
ffiCallbackOpen(environment, site, scheduler, lifetime, thread)
    -> { registration, context }
ffiCallbackEnter(context, site) -> { transition, environment }
ffiCallbackLeave(transition)
ffiCallbackClose(registration, context, site)
```

Open is failure-atomic, retains the environment before publishing the context,
and rolls the root back on failure. Enter validates all identity and policy
facts, establishes `Managed` state, resolves the current environment root, and
only then marks the entry active. Leave restores the exact prior foreign state
or detaches an entry-created binding on every exit. Close invalidates the
context before root release and does not consume a live registration on an
active-entry failure.

Native ABI 1.18 adds these exact operations:

```text
pop_rt_ffi_callback_open(
    environment: u64,
    site: u64,
    scheduler: u32,
    lifetime: u8,
    thread: u8,
    outContext: *mut u64
) -> u64

pop_rt_ffi_callback_enter(
    context: u64,
    site: u64,
    outEnvironment: *mut u64
) -> u64

pop_rt_ffi_callback_leave(transition: u64) -> u8

pop_rt_ffi_callback_close(
    registration: u64,
    context: u64,
    site: u64
) -> u8
```

Open returns a nonzero registration or zero and leaves `outContext` unchanged
on failure. Enter returns a nonzero transition or zero and leaves
`outEnvironment` unchanged on failure. Status one is the sole successful leave
or first close result; stale or duplicate native close returns zero. The ABI
uses `u64` for opaque tokens, not for arbitrary pointer
arithmetic; LLVM converts the validated context token to the callback's target
userdata pointer type at the foreign boundary. Native ABI 2 uses the same
logical callback state and still requires its existing writable-root
capability for collecting safe points inside managed callback execution.

## Consequences

- Capturing C callbacks use the familiar function-pointer/userdata convention
  without exposing a Pop object address.
- The common call-scoped form is concise while owned registration makes longer
  native lifetimes and cleanup explicit.
- The first stable policy excludes some concurrent or reentrant C APIs until
  their capture safety can be proven statically.
- Abort-on-panic is intentionally strict but cannot fabricate a value or unwind
  through an incompatible native frame.
- Backends share callback lifetime, identity, thread, and failure semantics;
  only the physical thunk calling convention is target-specific.

## Alternatives considered

### Pass a Pop closure address as userdata

Rejected because closures are managed objects whose address and layout are not
an FFI contract and may change during collection.

### Store the callback in a global table keyed by a string or symbol name

Rejected because runtime name resolution is reflection and an operational
dynamic escape hatch. The site and registration identities are closed typed
tokens.

### Permit a scoped callback function pointer without its context provenance

Rejected because native code could mix registrations or retain one half of the
pair. The compiler tracks the pair as one lexical capability.

### Allow panic to unwind into C by default

Rejected because most C frames have no compatible unwind contract and a panic
payload has no ABI representation.

### Make all callbacks concurrent and reentrant

Rejected because ordinary mutable captures do not yet have a static contract
that makes overlapping execution sound. Runtime locking alone would hide
semantics and can deadlock on reentry.

## Required conformance tests

- exact callback/context source signatures, one-context rule, ABI type mapping,
  immediate closure, capture, result, async, variadic, managed-parameter, and
  C-unwind negatives;
- trusted callback-pair metadata identity, indices, signature fingerprint,
  lifetime, thread, shadowing, malformed, and `.poplib` round-trip tests;
- call-scoped pair dominance, exact foreign argument use, return/store/capture/
  separate-call/address/suspension escape negatives, and every-exit close;
- owned open/withPair/close, alias-visible lifecycle state, closed-withPair
  result, pair lexical-escape negatives, source-idempotent/native-single-use
  close, active close failure, and unregister-before-close wrapper tests;
- HIR/MIR identities, signature preservation, nested callback body, verifier
  corruption, optimization preservation, and explicit interpreter/C capability
  tests;
- PLRI zero/nonzero, failure-atomic outputs, root retention/relocation, stale,
  forged, wrong-site, wrong-thread, overlapping, reentrant, duplicate leave,
  and wrong-transition tests;
- native same-thread foreign-to-callback-to-managed state restoration and
  attached-thread balanced attach/detach tests, plus bounded-nonblocking entry
  rejection before managed execution;
- LLVM typed-thunk calls against a deterministic C fixture, context round-trip,
  capture relocation, normal and expected-failure results, panic containment,
  and post-close violation tests;
- architecture regressions forbidding managed object addresses, runtime symbol
  lookup, unrestricted indirect signatures, callback unwind, implicit
  concurrency, and finalizer-based close.

## Documents/components affected

Type system, trusted FFI metadata, HIR, MIR, PLRI/runtime, collector state,
native ABI, LLVM, MIR interpreter, `Pop.Ffi`, `.poplib`, generated binding
metadata, diagnostics, conformance policy, and implementation roadmap.
