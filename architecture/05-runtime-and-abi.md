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

Under ADR 0059, ABI 1 native execution no longer uses `BootstrapRuntime`.
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

ADR 0060 advances native ABI 1 to version 1.11 with atomic initialized object
allocation. LLVM passes the exact pointer map and one physical initializer per
logical slot in a single native transition. The runtime validates every managed
initializer before publication and returns either a completely initialized
object or the closed allocation-failure sentinel. MIR retains one backend-neutral
typed record/class construction operation; later mutation still uses the
ordinary checked store and barrier path.

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

Pop GC is a precise concurrent generational collector. The compiler/runtime
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

Root publications preserve canonical sorted `RootSlot` order. A safe point
validates all roots and either completes every root/object/handle update or
fails without exposing a partial relocation. Bootstrap adapters leave tokens
unchanged; relocating adapters invalidate evacuated tokens after rewriting all
live locations.

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
capability from implementation experiments. ADR 0059 permits a closed native
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

The FFI is an explicit unsafe boundary. It needs a separate type mapping and
cannot treat Pop Lang objects as stable C structs unless a type opts into a
compatible representation.

FFI declarations remain statically typed. Untyped external bytes, handles, and
pointers must be decoded, wrapped, or used through explicit unsafe typed APIs;
the FFI does not introduce dynamic Pop Lang values.

FFI declarations should state:

- external ABI and symbol;
- parameter and result layouts;
- nullability and string encoding;
- ownership and lifetime rules;
- blocking, callback, and unwind behavior;
- whether the collector may move referenced values.

## Versioning

Artifacts record the language version, MIR version when serialized, PLRI ABI
version, target triple/capabilities, and enabled unstable features. ABI changes
must be detectable at load or link time rather than causing silent corruption.
