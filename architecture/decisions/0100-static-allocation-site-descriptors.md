# ADR 0100: Static Allocation-Site Descriptors

- Status: accepted
- Date: 2026-07-23
- Depends on: ADR 0038, ADR 0039, ADR 0070, ADR 0072, ADR 0078, ADR 0080,
  and ADR 0085
- Supersedes: the per-call native pointer-map transport portion of ADR 0072;
  its atomic unpublished-initialization semantics remain unchanged

## Context

Canonical MIR already gives every managed-capable construction one stable
`AllocationSiteId` and carries the exact semantic type, allocation class,
logical slot count, and precise object map. Native ABI 1.19 nevertheless passes
the slot count and a newly materialized reference-slot array for each
initialized record or class allocation. The native facade reconstructs,
sorts, validates, and clones an `ObjectMap` on every call. Later access,
barrier, and tracing paths search another per-object copy even though
monomorphic pages can share one immutable layout.

This repeated work is not semantic dynamism. It is a backend/runtime contract
gap. Closing it must not expose raw managed addresses, add name-based
resolution, make LLVM layout part of MIR, or let an untrusted mutable
descriptor silently redefine an allocation site.

## Decision

Every managed-capable construction MIR instruction carries its existing stable
`AllocationSiteId` through optimization. Its exact typed layout remains
backend-neutral MIR data. A backend may emit one immutable private
allocation-site descriptor for each retained managed allocation site.

PLRI defines a typed `AllocationSiteDescriptor` containing:

- the owning Bubble, owning callable, and callable-local stable
  `AllocationSiteId`;
- the exact `RuntimeTypeId`;
- the closed `AllocationClass`;
- the logical slot count; and
- one canonical immutable `ObjectMap`.

The descriptor is an implementation fact. It is not a Pop value, runtime
reflection object, public reference-metadata entry, symbol name, string key, or
unchecked extension point. MIR interpreters and future VM implementations
consume the same semantic fields without depending on the native physical
spelling.

Native ABI 1 advances from 1.19 to 1.20 with
`pop_rt_allocate_initialized_object_at_site`. The native physical descriptor is
a fixed-width read-only record containing the Bubble, callable, local
allocation-site ID, runtime type, allocation class, slot count, reference-slot
count, and a pointer to immutable compiler-emitted `u32` reference slots. The
call separately receives only the ordered physical initializer words and their
count. ABI 1.20 retains the ABI 1 stable-token collector profile.

On first use, the native runtime validates all descriptor widths, the closed
allocation class, canonical increasing in-bounds reference slots, initializer
count, and non-null managed initializer tokens. It interns the resulting typed
layout by allocation-site identity. Reuse of an identity with different
descriptor address or content is a runtime invariant failure. Later calls reuse
the validated immutable layout and never sort or clone its reference slots.
The legacy ABI 1.19 initialized-allocation entry remains available for already
generated objects but new ABI 1.20 LLVM output uses the site entry.

The collector stores one shared layout descriptor per monomorphic page.
Objects on the page retain only a shared descriptor reference; access,
barriers, and tracing consume that page-shared descriptor. Object placement
does not carry another type, size, domain, or pointer-map copy.

Object-map membership must be constant time after first validation. The
canonical ordered reference-slot projection remains available for precise
iteration, while an immutable compact membership bitmap or equivalent
constant-time representation serves checked access and barriers. This is typed
layout metadata, not a dynamic fallback.

Allocation-site telemetry keys survival and copy cost by
`AllocationSiteId`. Adaptive pretenuring may change only the closed
`AllocationClass` selected for later managed allocations at that site; it
cannot change semantic type, object map, initialization, ownership, or
visibility. Thresholds, decay, and memory-pressure policy remain collector
configuration and must be deterministic in conformance tests.

Native ABI 2 remains the separately negotiated writable-root descriptor from
ADR 0078. Its production successor must include the same allocation-site
semantics, but this ADR does not relabel an incomplete ABI 2.0 runtime as
production capable.

## Consequences

- LLVM emits immutable layout data once instead of stack-building pointer maps
  for every allocation.
- First use pays validation and interning; steady-state allocation reuses the
  page-shared precise layout.
- Allocation, access, barriers, tracing, and future pretenuring telemetry use
  one stable site/layout identity.
- Atomic initialization and unpublished-owner barrier elision from ADR 0072
  and ADR 0080 remain unchanged.
- ABI 1.20 is additive and exact. Older runtimes fail ABI negotiation before
  normal entry for newly generated code.
- Descriptor caches are typed internal runtime state. They do not permit
  lookup by source name, runtime mutation, arbitrary registration, or
  reflection.

## Alternatives considered

### Keep rebuilding `ObjectMap` per allocation

Rejected because the compiler already proved the layout and the repeated
sorting, validation, allocation, and cloning are avoidable common-path work.

### Pass raw page or payload addresses to LLVM

Rejected because managed addresses cannot escape PLRI and would break
relocation, safe-point, and backend-neutrality contracts.

### Use a string or nominal type name as the cache key

Rejected because runtime name resolution and string-keyed reflection are
forbidden. `AllocationSiteId` is the existing typed compiler identity.

### Trust arbitrary caller-owned descriptor memory forever

Rejected because mutation or identity aliasing could change precise tracing
after publication. Only compiler-emitted immutable descriptors are accepted,
and first-use validation plus identity consistency fails closed.

### Change ABI 2.0 in place

Rejected because ADR 0078 defines exact negotiation. A future production ABI 2
descriptor must advance deliberately and include the completed root,
unwind/coroutine/FFI, and collector proofs.

## Required conformance tests

- MIR verification requires one `AllocationSiteId` and one exact object map on
  every managed-capable construction and rejects duplicate or missing site
  identities after optimization.
- MIR text round trips preserve allocation-site identity and exact layout.
- PLRI descriptor construction rejects non-canonical, duplicate, and
  out-of-bounds reference slots.
- native ABI 1.20 maps the new operation to one unique symbol and rejects
  malformed descriptors, mismatched initializer counts, identity aliasing, and
  stale managed initializers without publishing an object.
- LLVM emits one immutable descriptor and reference-slot constant per retained
  site, passes no per-call pointer-map stack allocation, and negotiates ABI
  1.20.
- repeated native allocations at one site perform exactly one descriptor
  validation/intern event and share the page layout.
- scalar bits equal to managed tokens remain untraced under the descriptor
  membership representation.
- record/class initialization remains atomic and later mutation retains the
  precise checked barrier.
- the MIR interpreter and LLVM agree on values, traps, reachability, and
  initialization for scalar, mixed, pointer-free, and all-reference layouts.
- paired ADR 0099 `allocationChurn` and `objectArray` evidence rejects a
  checksum, median, P99, or peak-RSS regression before the implementation
  closes.

## Documents/components affected

Runtime and ABI architecture, MIR construction and verification, PLRI
allocation vocabulary, native-ABI version/symbol maps, native descriptor
validation/cache state, collector layout storage and membership checks, LLVM
constant emission, MIR interpreter consumption, conformance tests,
implementation roadmaps, and heap benchmark evidence.
