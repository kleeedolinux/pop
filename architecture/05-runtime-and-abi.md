# Runtime and ABI

## Runtime boundary

Generated code depends on a small, versioned **Pop Lang Runtime Interface**
(PLRI). PLRI describes semantics in backend-neutral terms. Each backend maps the
interface to its execution environment.

LLVM-generated native code may call a C-compatible runtime ABI. A future VM may
implement the same operations directly in its interpreter. MIR refers to PLRI
operations, never to C symbol names.

[ADR 0038](./decisions/0038-modular-portable-runtime-implementation.md)
separates the Rust implementation into four ownership boundaries:

- `pop-runtime-interface` owns only backend-neutral PLRI values, operations,
  maps, failures, and adapter traits;
- `pop-runtime-collector` owns the reusable collector engine and has no C ABI,
  platform entry, or process-global runtime responsibility;
- `pop-runtime-native-abi` owns the versioned C spelling and physical sentinel
  vocabulary used by native adapters;
- `pop-runtime-native` owns exported symbols, process-global stable-token
  generational state,
  UTF-8/process-entry adaptation, and native termination behavior.

The Cargo dependency direction is native facade → collector/native-ABI → PLRI.
The collector and native-ABI crates depend on PLRI independently; neither
depends on the other. A VM can therefore compose the collector without linking
the native C ABI, while LLVM can select native ABI symbols without importing
collector storage or tracing policy.

Native ABI 1 uses nonzero `u64` opaque managed/root tokens and
zero as the invalid/null sentinel. The LLVM backend chooses this physical
representation in its private lowering; MIR remains expressed in abstract
managed references and `RuntimeOperation` identities. Exported `pop_rt_*`
symbols are versioned and cover only operations with a defined ABI
failure sentinel; richer failures remain typed `RuntimeFailure` values in PLRI
and Rust adapters.

ADR 0039 fixes this as the `BootstrapStableHandles` profile only. A PLRI
`ManagedReference` is the current opaque physical token, not source-visible
identity or a stable address. Collecting safe points receive a mutable typed
`RootPublication`; a moving collector replaces relocated tokens in their exact
`RootSlot`s before returning. Strong root/pin handles remain distinct stable
runtime tokens whose targets are updated internally.

The first production moving native runtime uses ABI major version 2 and requires
a backend with verified relocating-root support. ABI 1.x, immutable root spills,
or a target capability alone cannot satisfy the production profile. Profile/
ABI mismatch fails before link or load; there is no silent bootstrap fallback.

[ADR 0078](./decisions/0078-native-abi-2-writable-root-coexistence.md)
keeps ABI 1.11 and ABI 2.0 as distinct closed descriptors. ABI 1 retains
`pop_rt_gc_safe_point`; ABI 2 uses `pop_rt_gc_safe_point_v2` with an exact
writable root array and reloads every returned slot before any later managed
use. `pop_rt_supports_abi(major, minor)` is a defensive typed load/link check,
not runtime symbol selection. [ADR 0079](./decisions/0079-native-task-frame-and-cancellation-abi.md)
adds the coexisting ABI 1.12 stable-token descriptor for compiler-created task
frames and distinct cancellation source/token authority. A conforming 1.12
facade continues to support 1.11, and neither descriptor may report ABI 2
until the complete moving composition exists.

Under ADR 0070, ABI 1 native execution no longer uses `BootstrapRuntime`.
Instead it composes the generational allocator and incremental SATB mature
collector in a `NativeStableGenerationalConformance` stage that places every
native allocation in a non-moving domain. This stage preserves ABI 1 stable
tokens and cannot enable the moving nursery or selective evacuation. ABI 2 and
verified post-safe-point LLVM root reloads remain mandatory for the production
profile.

`Pop.Internal` supplies the trusted managed/intrinsic side of these contracts;
`Pop.Standard` calls public typed adapters rather than PLRI entries directly.
See [Base libraries](./16-base-libraries.md).

The standalone native executable links Rust static archives for both foundation
libraries. Trusted `Pop.Standard` prelude-function identities map the typed
source calls `print(Int)` and `print(String)` to fixed output adapters. These
adapters are not PLRI operations, and their host ABI spellings never
participate in source name resolution.

The native runtime supplies a versioned, read-only UTF-8 string-copy ABI for
the trusted string-output adapter. It validates the managed `String` identity,
distinguishes an empty string from failure, and rejects undersized buffers.
This addition advanced native ABI 1 from version 1.0 to 1.1.
`Pop.Internal` contains the unsafe ABI call and exposes a checked adapter to
`Pop.Standard`; generated MIR cannot inspect string storage or call the ABI by
spelling. This service does not expose general object memory,
reflection, or string mutation.

String concatenation and closed primitive formatting use typed PLRI semantics.
The native runtime may expose versioned private helpers to materialize their
owned UTF-8 results, but HIR/MIR name only backend-neutral `StringConcat` and
`StringFormat` operations. Helpers never inspect a runtime type or ambient
locale. Their output bytes follow ADR 0041 identically across backends.

Scoped pin handles advance native ABI 1 to version 1.2. Distinct
typed table allocation advances it to version 1.3: the allocation request and
native entry carry the entry count plus homogeneous key/value managed-reference
maps. Arrays retain their contiguous homogeneous element map, while tables use
interleaved key/value slots without becoming generic objects or exposing their
layout to MIR.

ADR 0034 advances native ABI 1 to version 1.4 and completes fixed
arrays with bulk initialized allocation, O(1) length, optional and checked
reads, checked writes, and fill. PLRI adapters preserve one-based bounds
behavior and distinguish scalar from managed elements. Native backends may
scalar-replace non-escaping arrays, batch transitions, or use scoped pinned
contiguous access, but raw managed pointers never become source, HIR, MIR, or
public PLRI values.

ADR 0041 advances native ABI 1 to version 1.5 with closed string
concatenation and primitive-format helpers. The format tag is selected from the
verified MIR operand kind and cannot request a runtime type lookup or universal
formatting fallback.

ADR 0046 advances native ABI 1 to version 1.6 with typed table get
and insert-or-replace operations. Key comparison follows the compiler-approved
canonical key contract, and table growth preserves stable managed identity and
precise key/value maps.

ADR 0051 advances native ABI 1 to version 1.7. Optional array and
table lookup adapters return a presence status separately from an out payload,
so a present scalar zero or `false` cannot collide with absence. LLVM's private
typed optional pair is not a MIR or PLRI value representation.

ADR 0053 advances native ABI 1 to version 1.8 with statically
selected iteration-session acquisition and step operations. The acquisition
operation receives a compiler-proven closed collection-kind tag. The step
operation returns a closed item/end status plus one typed raw payload; tuple
items remain ordinary typed tuple objects. Iterator state roots its source,
checks the source length or key-set size before every step, and never performs
member lookup from a string.

The growable-list portion of ADR 0053 advances native ABI 1 to
version 1.9. `ListCreate` receives a nonnegative reserved capacity and the
compiler-proven homogeneous element-reference map, returning a stable managed
handle or the closed allocation-failure sentinel. `ListLength`, optional and
checked `ListGet`, checked `ListSet`, and `ListAdd` use status-plus-out-payload
adapters where a scalar zero is a valid element. The native facade keeps length
and capacity private, grows storage without changing the list handle, and
applies precise barriers for managed elements. MIR retains distinct typed list
operations; no backend may reinterpret them as array or table operations.

ADR 0072 advances native ABI 1 to version 1.11 with atomic initialized object
allocation. LLVM passes the exact pointer map and one physical initializer per
logical slot in a single native transition. The runtime validates every managed
initializer before publication and returns either a completely initialized
object or the closed allocation-failure sentinel. MIR retains one backend-neutral
typed record/class construction operation; later mutation still uses the
ordinary checked store and barrier path.

ADR 0079 advances native ABI 1 to version 1.12 with opaque compiler coroutine
frames, cold task creation, direct and structured ownership transfer, explicit
cancellation source/token authority, and exact task completion retention. Task
creation and task-group wrapping receive the compiler-proven managed-completion
flag, so the task control object precisely maps its completion and cancellation
token slots. Cold frame roots remain retained until native scheduler admission
overlaps them with the ready-frame publication; admission rejection restores
the unpolled ownership state atomically. Structured groups retain every owned
child until join, while unreachable cold/terminal side records are weakly
pruned after collection without adding source-visible finalization.

ADR 0081 advances native ABI 1 to version 1.13 with balanced foreign-call
transitions. `EnterForeign` services one mutable precise root publication and
returns a thread-bound, LIFO, single-use `ForeignTransitionId`; `LeaveForeign`
restores managed state, writes current roots into the identical publication,
and consumes that identity. Potentially blocking calls retain their roots in
runtime-owned strong handles and transition to `HandlesOnly`; exact reviewed
nonblocking calls use `BoundedForeign`. The native entries are
`pop_rt_enter_foreign` and `pop_rt_leave_foreign`; their writable arrays are
new ABI 1.13 arguments and do not alter the immutable ABI 1 safe-point entry.
ABI 2 preserves the same PLRI operation while additionally proving relocating
writeback on return and unwind.

The same ADR advances native ABI 1 to version 1.14 with balanced managed-thread
attachment. `AttachManagedThread` registers an exact scheduler/mutator binding
in managed state and returns a thread-bound `ManagedThreadBindingId`;
`DetachManagedThread` requires no active native transition, detaches,
unregisters, and consumes that identity. The generated native program entry
attaches logical scheduler 1 before argument decoding or Pop invocation and
detaches after normal return. Scheduler workers retain their existing
dispatch-owned binding rather than attaching again.

At an argument-taking binary boundary, the target entry adapter omits the
executable path, validates each remaining platform argument as UTF-8, and
constructs the canonical managed `Array<String>` before invoking the entry
`SymbolId`. A no-argument entry does not decode platform arguments. The
array has a precise managed-reference element map and is rooted while its
strings are materialized. Empty and non-ASCII arguments are preserved exactly.
Invalid UTF-8 causes a closed runtime trap before user code executes; the
adapter never performs lossy conversion. An entry's `Int` result becomes the
platform process status; normal completion of a no-result entry becomes status
zero. This target-specific adapter does not change MIR's logical Pop Lang
calling convention.

## Runtime responsibilities

The runtime is expected to own or coordinate:

- heap allocation and collection;
- strings and string interning where appropriate;
- typed tables and collection primitives;
- runtime type information needed for checked operations;
- interface and virtual dispatch metadata;
- panic/error propagation and stack traces;
- coroutine/task scheduling hooks;
- foreign-function transitions;
- Module and Bubble initialization state;
- explicitly retained metadata projections and their generated typed adapters.

Arithmetic on unboxed primitives, direct calls, fixed field access, and other
simple operations should not require runtime calls.

The closed portable trap vocabulary includes `NumericConversion` for a checked
integer target that cannot represent its input. Backends perform the conversion
directly where possible, but must raise that same trap for out-of-range integer
casts and for NaN, infinity, or out-of-range float-to-integer casts. See ADR
0040. Numeric `for` raises the closed `InvalidRangeStep` trap before iteration
when a dynamic step is zero; a backend cannot treat it as an empty range or
choose a direction. See ADR 0042.

## Object model

An ordinary class instance has a backend-selected header followed by declared
storage. Its semantic descriptor includes class identity, field descriptors,
implemented interfaces, and method dispatch information.

The language specifies observable behavior, not a fixed header layout. A native
backend might use a type-information pointer and GC bits; a VM might use an
internal object handle. Both must agree on field initialization, identity,
type tests, dispatch, and explicitly retained metadata behavior.

Internal type metadata used for GC, dispatch, or a checked cast is not language-
level reflection and is not automatically accessible to a program.

## Calling convention

MIR calls use a logical Pop Lang calling convention. Backends lower it to their own
physical conventions.

The logical convention describes:

- receiver placement for methods;
- argument and return types;
- tuple/multiple-result behavior;
- generic type or dictionary arguments when required;
- closure environment arguments;
- error, unwind, and suspension behavior;
- ownership/rooting expectations across the call;
- whether the call is a GC safe point.

Each call carries the canonical closed effect summary, and every `MayUnwind` MIR
instruction carries an explicit unwind action. An unwind action either
propagates panic to the caller or enters a verified cleanup block that
eventually continues normal control flow or resumes unwinding through canonical
MIR's `resumeCurrentUnwind` terminator. Cleanup blocks carry their typed lexical
scope identity and closed exit reason.
Runtime traps use a closed `TrapKind` and do not become
catchable expected errors.

A panic raised while panic cleanup is already active becomes the non-allocating
closed `PanicKind.DoublePanic`. It stops unwinding and reaches the nearest
task/process panic boundary as a terminal condition; neither original payload
is exposed as runtime reflection or nested into the terminal record. See ADR
0052.

Expected failure propagation is ordinary typed control flow, not unwinding.
Its MIR failure edge and normal returns traverse the same active lexical
cleanup chain as panic/cancellation exits, with an explicit exit reason and
destination. Cleanup order is last-in, first-out and backend-neutral. See ADR
0052.

Native code can use platform ABI conventions at foreign and public boundaries
without forcing those conventions onto internal MIR.

## Garbage collection contract

Pop uses proof-directed static reclamation before Pop GC. `Elided` and fixed
activation-owned storage require no PLRI allocation; compiler-inferred scoped
regions use bounded typed open/allocate/close operations and exact outward
managed roots. Every plan and lifetime frontier is fixed in verified optimized
MIR. PLRI exposes no machine-stack address, compiler arena object, or raw
`malloc`/`free` spelling. See
[Static memory management](./24-static-memory-management.md).

Pop GC is the precise concurrent generational fallback. The compiler/runtime
contract exposes:

- allocation classes;
- precise stack/object root maps;
- safe points;
- SATB and generational card write barriers;
- handles and scoped pinning for foreign calls;
- stack scanning and coroutine stack ownership.

PLRI represents these with backend-neutral allocation requests, logical object
maps, safe-point/stack-map descriptors, managed handles, root and pin
transitions, reference-store barriers, trap kinds, and panic/unwind records. It
does not expose compiler arenas, LLVM values, C symbol names, or raw managed
pointers.

Static slots and scoped regions that contain managed references publish exact
mutable root slots until their verified end/close. Managed objects cannot point
into that storage. Missing retention/lifetime proof selects an ordinary managed
allocation rather than an unchecked static path.

Root publications preserve canonical sorted `RootSlot` order. A safe point
validates all roots and either completes every root/object/handle update or
fails without exposing a partial relocation. Bootstrap adapters leave tokens
unchanged; relocating adapters invalidate evacuated tokens after rewriting all
live locations.

Under ADR 0077, the scheduler retains the same canonical publications for every
non-running task frame, including ready frames. Collector-owned typed root
containers keep those slots live and relocation-updatable; dispatch restores
the current tokens before polling. Worker mutator identity and scheduler
selection are bound atomically to each native runtime operation, never inferred
from one process-global semantic scheduler.

Collector implementation stages are explicit. `RelocationConformance` provides
the first single-mutator moving nursery and generational card barrier, but no
mature-heap collection, concurrent marking, or SATB barrier; mature objects are
retained. It is therefore test infrastructure for relocation correctness, not
the selectable `ProductionGenerational` runtime profile.

The collector also contains a later `GenerationalRuntime`
conformance composition. It adds page-described TLAB placement, cooperative
incremental SATB mature tracing/sweeping, protected emergency and evacuation
reserves, typed non-heap memory accounting, adaptive growth targets, bounded
allocation assists, deterministic byte-limit OOM, empty-page return, and
domain/debt telemetry. It still reports the lower relocation contract because
cooperative work is not concurrent production marking, the native backend does
not yet provide writable relocating roots, and no profile may infer production
capability from implementation experiments. ADR 0070 permits a closed native
stable-token wrapper to use its mature allocator, SATB marking, and sweeping
without exposing nursery relocation; this does not select the production
profile.

The collector implementation consumes these contracts. It does not redefine
them, and native exports remain a delegating facade over a concrete collector.
Crate separation adds no runtime registry, string lookup, or hot-path dynamic
dispatch.

The Milestone 3 bootstrap runtime implements a safe precise stop-the-world
handle-table mark/sweep heap. It is the executable proof for allocation, object
maps, roots, safe points, reachability, and deterministic out-of-memory
behavior; it is not labeled as the production moving/concurrent collector.

The production design uses a moving nursery and mostly non-moving mature heap;
user finalizers and weak references are excluded from version one. Full details
are in [Garbage collector architecture](./15-garbage-collector-architecture.md).

## Runtime metadata and reflection

Runtime reflection is absent by default. The compiler may still emit private
metadata required for collection, dispatch, stack unwinding, and checked type
tests; programs cannot enumerate or index that metadata.

The explicit `@RetainMetadata` attribute may request a narrow public metadata
projection.
Even then, the runtime does not expose a dynamically typed `get(name)` or
`call(name, args)` API. Instead, compile-time analysis generates a statically
typed adapter for the declared use case, such as serialization, RPC binding, or
test discovery.

Runtime metadata follows these rules:

- no process-wide “all types” registry is required;
- private members stay inaccessible unless their own declaration explicitly
  opts into a named compiler-supported capability;
- field values are never returned as `any`, `dynamic`, or an untyped box;
- mutation cannot bypass visibility, immutability, or invariants;
- lookup by arbitrary runtime string cannot resolve a program symbol;
- UDAs are compile-time-only unless a retention policy explicitly serializes a
  permitted data projection;
- retained metadata is removable by dead stripping when its adapter is unused.

## Module and Bubble initialization

Compiled library Bubbles expose a manifest plus Module initialization entry
points. A `BubbleContext` tracks `Unloaded`, `Loading`, `Loaded`, `Initializing`,
`Ready`, and `Failed`. Runtime initialization cycles are errors. Failure is
cached; retry requires a new context in version one.

Pure constants and type metadata should be loadable without executing arbitrary
module code. See
[Bubbles, namespaces, artifacts, and loading](./14-libraries-namespaces-and-loading.md).

## Foreign-function interface

The stable FFI follows
[ADR 0081](./decisions/0081-statically-bound-native-ffi.md). It is an explicit
unsafe, statically bound boundary with a separate closed ABI type mapping.
Ordinary namespace functions carrying the exact trusted `Ffi.Foreign` identity
declare external symbols; namespace `Ffi.Link` attachments refer to
typed `bubble.toml` native-library aliases. No `lib` runtime container,
untyped linker flags, shell command substitution, runtime symbol lookup, or
dynamic Pop Lang value exists.

Canonical HIR/MIR retains the resolved foreign identity, ABI, exact layout,
effects, and ownership/rooting facts. Every call performs backend-neutral
`enterForeign`/`leaveForeign` transitions, publishes precise roots, and is a GC
safe point. Blocking is the safe default; native unwind and callbacks require
exact explicit contracts.

Raw pointers refer only to unmanaged ABI storage or a compiler-verified lexical
borrow of an exact storage payload. ADR 0082 closes the first managed pin to
immutable `Bytes` and returns a read-only payload pointer plus exact length;
arrays, classes, strings, closures, and ordinary records never expose object
addresses. `Ffi.Buffer<T>` owns bounds-checked, aligned, zero-initialized
unmanaged ABI storage with deterministic close. Managed references cross
longer boundaries as generation-checked `Ffi.Handle<T>` tokens or copies.
Fixed-layout C records opt in through `Ffi.C.Layout` and are marshalled to
separate ABI storage; unannotated Pop objects never become C structs. Strings
use explicit encoding and ownership adapters. Generated bindings are
deterministic reviewable source plus hashed ABI metadata, and safe public
wrappers convert those declarations into normal typed Pop APIs. See
[ADR 0082](./decisions/0082-ffi-abi-storage-and-lexical-borrows.md).

Native ABI 1.15 adds `pop_rt_resolve_root` for the exact current target of a
live generation-checked handle. Creation/release retain their existing
retain-root/release-root entries. Invalid, stale, forged, zero, or closed
handles fail before a managed reference is returned.

Native ABI 1.16 adds the exact managed-resource operations for
`Ffi.Buffer<T>`. Open distinguishes allocation failure, success, and invariant
failure; all other operations use checked status plus unchanged-on-failure
outputs. Buffer reads and writes use the same one-based indexing contract as
Pop collections. Buffer state follows collector relocation while borrowed
addresses refer only to its separately owned ABI storage. See
[ADR 0083](./decisions/0083-ffi-resource-state-and-native-buffer-abi.md) and
[ADR 0084](./decisions/0084-canonical-mir-ffi-buffer-operations.md).

Native ABI 1.17 adds `pop_rt_ffi_bytes_borrow` and
`pop_rt_ffi_bytes_end_borrow`. The runtime atomically pins the exact immutable
`Bytes` owner and returns only a null-or-nonzero payload address plus exact
length and a nonzero private token. Failure leaves outputs unchanged. The
compiler and backends never calculate a payload offset from managed object
layout. Scoped buffer and byte borrows execute only immediate synchronous
closures through one verified MIR region call. See
[ADR 0087](./decisions/0087-scoped-ffi-borrow-bodies-and-bytes-pin-abi.md).

Native ABI 1.18 adds failure-atomic callback open, enter, leave, and close
operations. A registration roots one exact typed managed environment while its
native-visible context is only a runtime-owned opaque address token. Each fixed
backend thunk embeds a `FfiCallbackSiteId`; enter validates that site, the
context generation, lifetime, creating scheduler/thread policy, and serialized
non-reentrant state before returning the current environment reference and
establishing managed execution. Leave restores the exact prior foreign state or
detaches an entry-created binding. Close invalidates the context before
releasing the root and fails while an entry is active. Callback panic and every
invalid entry are contained at the generated panic boundary and never unwind
through foreign frames. See
[ADR 0092](./decisions/0092-typed-ffi-callbacks-and-native-transition-abi.md).

The compact nonzero `FfiAbiLayoutId` used by those operations is the first
eight big-endian bytes of ADR 0086's full canonical SHA-256 layout fingerprint.
Artifacts and generated metadata retain and compare the full fingerprint and
all descriptor facts; zero or a compact collision between unequal full
fingerprints fails before native execution.

## Versioning

Artifacts record the language version, MIR version when serialized, PLRI ABI
version, target triple/capabilities, and enabled unstable features. ABI changes
must be detectable at load or link time rather than causing silent corruption.
