# Runtime and ABI

## Runtime boundary

Generated code depends on a small, versioned **Pop Lang Runtime Interface**
(PLRI). PLRI describes semantics in backend-neutral terms. Each backend maps the
interface to its execution environment.

LLVM-generated native code may call a C-compatible runtime ABI. A future VM may
implement the same operations directly in its interpreter. MIR refers to PLRI
operations, never to C symbol names.

The bootstrap native ABI uses nonzero `u64` opaque managed/root handles and
zero as the invalid/null sentinel. The LLVM backend chooses this physical
representation in its private lowering; MIR remains expressed in abstract
managed references and `RuntimeOperation` identities. Exported `pop_rt_*`
symbols are versioned and cover only operations with a defined bootstrap
failure sentinel; richer failures remain typed `RuntimeFailure` values in PLRI
and Rust adapters.

`Pop.Internal` supplies the trusted managed/intrinsic side of these contracts;
`Pop.Standard` calls public typed adapters rather than PLRI entries directly.
See [Base libraries](./16-base-libraries.md).

The standalone native bootstrap links Rust static archives for both foundation
libraries. A trusted `Pop.Standard` prelude-function identity maps the typed
source call `print(Int)` to a fixed integer-output adapter. This adapter is not
a PLRI operation, and its host ABI spelling never participates in source name
resolution.

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

Each call also carries the canonical closed effect summary and an explicit
unwind action. An unwind action either propagates panic to the caller or enters
a verified cleanup block that eventually continues normal control flow or
resumes unwinding. Runtime traps use a closed `TrapKind` and do not become
catchable expected errors.

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
