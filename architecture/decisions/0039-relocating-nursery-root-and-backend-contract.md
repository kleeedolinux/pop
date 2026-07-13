# ADR 0039: Relocating Nursery Root and Backend Contract

- Status: accepted
- Date: 2026-07-12
- Depends on: ADR 0001, ADR 0008, ADR 0018, ADR 0022, ADR 0024, and ADR 0038
- Supersedes: none

## Context

Pop GC requires a moving young generation, but the executable bootstrap uses
stable opaque handles. The current PLRI `RuntimeAdapter::safe_point` borrows a
`RootPublication` immutably. LLVM spills root values before the bootstrap call
but does not reload them afterward. That is sufficient for precise reachability
when handles never change; it cannot update relocated stack/register roots.

Treating handle-table compaction as a moving nursery would preserve the old
physical tokens and fail to prove relocation. Conversely, exposing raw pointers
through PLRI would make MIR and alternate backends depend on a native layout.
The production boundary needs to state how physical references change while Pop
Lang object identity and backend-neutral MIR remain stable.

Target metadata already names precise stack maps and relocating-nursery support,
but there is no implemented backend capability contract that prevents a driver
from selecting a production collector with a stable-handle-only lowering.

## Decision

### Semantic identity and physical references

Pop object identity is semantic and remains stable across relocation. A PLRI
`ManagedReference` is an opaque backend/runtime token for the current physical
reference, not source-visible identity, a stable address, or a serializable
handle. Every live token may be replaced at a collecting safe point.

`RootHandle` and `PinHandle` remain distinct stable runtime-private tokens. The
collector updates a strong handle's target internally. Pinning a young object
promotes it to non-moving storage before a stable address can cross an unsafe
boundary; a pin token is not the managed reference itself.

### Mutable precise root publication

`RootPublication` retains one canonical `StackMap` and one optional managed
reference for each sorted `RootSlot`. It exposes typed immutable and mutable
slot/value iteration in that exact order.

`RuntimeAdapter::safe_point` accepts `&mut RootPublication`. A relocating
collector must, before returning successfully:

1. validate every published token;
2. evacuate/promote reachable young objects;
3. update managed fields, strong handles, runtime roots, and pins;
4. replace every changed stack/root token in the publication;
5. make old evacuated tokens invalid; and
6. return only after the complete root/object update is visible atomically to
   the stopped mutator.

The bootstrap collector validates the same mutable contract but leaves root
tokens unchanged. A runtime error cannot expose a partially updated
publication. Root updates are typed by `RootSlot`; there is no raw byte scan,
runtime name lookup, conservative fallback, or untyped relocation table.

The MIR interpreter writes the returned root values back to the corresponding
live MIR values before executing the next instruction. A VM updates its typed
register/frame slots directly. MIR continues to describe semantic live roots;
it does not gain backend-specific pointer or relocation instructions.

### Runtime profiles and backend capabilities

The implementation distinguishes two closed runtime profiles:

- `BootstrapStableHandles`: precise roots and the ABI 1.x stable-handle
  representation; it never claims a moving nursery;
- `ProductionGenerational`: moving nursery plus the accepted mature-heap,
  barrier, handle/pin, and safe-point semantics.

`pop-backend-api` owns backend GC capability facts independently from target
facts. A backend states whether it supports precise roots and whether its
managed-reference lowering supports relocation. The selected target and backend
must both satisfy the requested runtime profile. Missing relocation capability
is a closed validation error before artifact emission; the driver never silently
downgrades production to bootstrap GC or emits no-op barriers/root updates.

The experimental C backend supports neither managed runtime profile. The LLVM
backend remains `BootstrapStableHandles` until verified statepoints/stack maps
or an equivalent writable-root lowering reloads every relocated value across
control flow. The future VM advertises relocation only after bytecode
verification and forced-minor tests prove register/frame updates.

### Native ABI and implementation staging

Native ABI 1.7 remains the bootstrap stable-handle ABI. It cannot be linked as a
production moving runtime. The first production native ABI uses major version
2 and either:

- passes writable precise root slots to the runtime and reloads them after the
  safe point; or
- uses verified LLVM statepoint/relocate machinery with an equivalent runtime
  handshake.

ABI-major/profile incompatibility is rejected by the toolchain before link or
load. Raw managed pointers still do not enter MIR, Pop source, public PLRI
values, or foundation-library metadata.

Implementation remains inside `pop-runtime-collector`. The first Stage-2 slice
is a single-mutator relocation conformance collector that really copies live
young objects, rewrites roots/fields/handles, invalidates old tokens, and proves
card-table behavior. It is not called the production parallel/TLAB collector.
TLABs, regions, parallel evacuation, adaptive sizing, and pause engineering
follow after relocation correctness passes.

The PLRI implementation contract represents this honestly as the distinct
`RelocationConformance` collector stage. That stage has precise mutable roots, a
moving nursery, and a generational card barrier, but it has no mature-heap
collection, concurrent marking, or SATB barrier. Mature objects are retained in
this conformance stage. It cannot satisfy the `ProductionGenerational` runtime
profile; the implementation-stage label and selectable runtime profile remain
different concepts.

## Consequences

- PLRI can express real relocation without exposing one backend's pointers.
- Existing runtime adapters must accept mutable root publications even when they
  do not move objects.
- Interpreter and VM execution must install updated root tokens before further
  managed operations.
- LLVM cannot claim production GC based only on pre-call root spills.
- Bootstrap ABI 1.x remains compatible within its explicitly limited profile.
- Stage-2 correctness can be tested independently of native statepoint and
  parallel-allocation performance work.

## Alternatives considered

### Keep immutable roots and return a relocation map

Rejected because consumers could omit updates, mappings could lose the exact
`RootSlot` relationship, and an extra general map invites untyped/token-based
lookup. In-place typed publication updates make the safe-point postcondition
explicit.

### Keep stable handles as all managed references in production

Rejected because it adds indirection to every access and does not implement the
accepted pointer-bump moving-nursery design. Stable handles remain appropriate
for FFI roots and the bootstrap profile.

### Put LLVM statepoint values in MIR or PLRI

Rejected because LLVM intrinsics and physical locations are backend-private.
MIR owns liveness and semantic safe points; each backend owns relocation
lowering.

### Enable production GC whenever the target lists relocation

Rejected because target feasibility does not prove that the selected backend
emits writable roots, reloads values, or handles control-flow merges correctly.

### Add a separate production-GC Cargo crate immediately

Rejected because ADR 0038 already establishes the collector implementation
boundary. Bootstrap and production implementations can remain focused modules
behind the same PLRI dependency until a real independent dependency boundary is
demonstrated.

## Required conformance tests

- mutable root iteration preserves canonical `RootSlot` order and cannot change
  root count or stack-map identity;
- bootstrap safe points preserve every root token exactly;
- a relocation test adapter rewrites roots and the MIR interpreter uses the new
  token after the safe point;
- the Stage-2 conformance collector copies live young objects, updates roots,
  object edges, strong handles, and pins, and rejects old tokens;
- unreachable young objects disappear and survivor age/promotion rules are
  deterministic;
- mature-to-young stores dirty cards, minor collection scans the remembered
  cards, and scalar/non-reference stores do not;
- forced minor collection at every eligible safe point preserves behavior before
  and after MIR optimization;
- backend capability/profile validation rejects C, stable-handle-only LLVM, or
  unsupported targets for `ProductionGenerational` without emitting artifacts;
- LLVM advertises relocation only after emitted stack-map/statepoint inspection
  plus native forced-relocation execution proves post-safe-point reloads;
- VM relocation capability requires typed frame/register update tests;
- ABI 1.x cannot satisfy the production profile and ABI-major mismatch fails
  before link/load;
- no conservative scan, read barrier, finalizer, weak reference, dynamic lookup,
  backend-specific MIR, or raw managed pointer escape is introduced;
- benchmark records distinguish bootstrap, relocation-conformance, and
  production collectors rather than comparing them as the same stage.

## Documents/components affected

Runtime and ABI, GC architecture, intermediate representations, backend
architecture/API, target capabilities, implementation roadmap, PLRI maps and
adapter trait, bootstrap/native runtime, MIR interpreter, LLVM backend, future
VM, architecture tests, forced-GC conformance, and benchmark profiles.
