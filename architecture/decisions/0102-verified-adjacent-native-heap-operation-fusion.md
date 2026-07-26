# ADR 0102: Verified Adjacent Native Heap-Operation Fusion

- Status: accepted
- Date: 2026-07-26
- Depends on: ADR 0034, ADR 0038, ADR 0039, ADR 0070, ADR 0072, ADR 0078,
  ADR 0080, ADR 0085, ADR 0099, ADR 0100, and ADR 0101
- Supersedes: none

## Context

Canonical MIR correctly keeps allocation, checked array mutation, checked array
access, and field access as distinct typed operations. Native ABI 1.20 lowers
each operation through a separate host call even when verified optimized MIR
proves that two adjacent operations form one non-escaping access chain. The
retained-object workload consequently crosses the native boundary four times
per object: allocate, store its token in a managed-reference array, reload the
token, and load one statically resolved field.

This cost does not authorize a benchmark-specific loop, raw managed address,
unchecked lookup, new MIR semantic operation, reordered trap, omitted safe
point, or backend-only language behavior. A native backend may combine physical
transitions only when the original canonical operations and every observable
boundary remain recoverable from the proof.

## Decision

Optimized native lowering may fuse exactly these adjacent verified MIR pairs:

- one atomically initialized fixed-layout object or class construction followed
  immediately by one checked managed-reference `ArraySet` whose stored value is
  that construction result; and
- one checked managed-reference `ArrayGetChecked` followed immediately by one
  statically resolved `FieldGet` whose base is that read result.

The paired intermediate result must have exactly one use, both instructions
must remain in one basic block, and no call, safe point, trap edge, root
publication, ownership transition, foreign transition, volatile observation,
or other instruction may occur between them. The exact array element map,
allocation-site descriptor, object map, physical field slot, evaluation order,
and result type are compiler-proven.

Fusion is a private native lowering plan. Canonical MIR, MIR text, the MIR
interpreter, future VM input, and source semantics retain the separate
operations. Native ABI 1 advances from 1.20 to 1.21 with two additive typed
adapters:

```text
pop_rt_allocate_initialized_object_at_site_and_store_array
pop_rt_array_get_object_field_checked
```

The first adapter performs the original complete object initialization before
the original checked array store. It returns the allocated stable token on
success and zero on allocation or checked-store failure. A failed checked store
does not pretend the preceding allocation did not occur; native lowering takes
the same trap edge as the unfused `ArraySet`. Unpublished-initialization
barrier elision and the checked managed-reference store capability remain those
of ADRs 0072, 0080, and 0101.

The second adapter performs the original one-based checked array load before
the original statically numbered field load. It reports success separately
from the field payload, so scalar zero remains distinguishable from failure.
Both owner and loaded target pass the same typed checked page access as the
unfused operations. A stale token, wrong allocation kind, out-of-bounds index,
invalid field, or invalidated direct span fails closed.

Both adapters accept opaque managed tokens and fixed typed descriptor/ordinal
arguments only. They expose no page address, payload pointer, runtime type
name, string lookup, reflection, dynamic operation, or unchecked fallback.
Relocation invalidation, writable-root reloads, safe-point placement, barriers,
ownership, and memory accounting remain unchanged.

An implementation may inline the two already-authorized operations inside one
native transition. It may not specialize for a source name, loop count,
benchmark checksum, concrete application, or current token value.

## Consequences

- Proven adjacent pairs pay one native transition instead of two.
- MIR stays backend-neutral and continues to govern interpreter, LLVM, and
  future VM semantics.
- Any intervening effect, additional use, control-flow edge, or missing type
  proof retains the ordinary unfused calls.
- ABI 1.21 is additive and exact; ABI negotiation rejects an older runtime
  before normal entry.
- This optimization does not enable native nursery relocation or make the
  production concurrent collector selectable.

## Alternatives considered

### Add fused operations to canonical MIR

Rejected because the existing operations already express the language
semantics and future runtimes need not share this native transition cost.

### Expose direct page pointers to LLVM

Rejected because raw managed addresses cannot cross PLRI, survive relocation,
or remain valid across safe points.

### Recognize the retained-object benchmark loop

Rejected because source-, workload-, or checksum-specific compilation is not a
language optimization contract.

### Fuse across safe points or effectful instructions

Rejected because doing so can reorder failure, root publication, collection,
ownership, or foreign observation.

## Required conformance tests

- LLVM emits each ABI 1.21 adapter for a positive exact adjacent pair and omits
  the two ordinary calls it replaces.
- LLVM retains ordinary calls when the intermediate value has another use, an
  instruction or control-flow edge intervenes, the array is not a managed-
  reference array, or the field is not statically resolved.
- the initialized values, one-based bounds behavior, array store, field result,
  scalar-zero result, and trap edge equal the unfused operations;
- malformed descriptors, stale tokens, wrong kinds, invalid indexes, invalid
  fields, and invalidated spans fail closed without unchecked access;
- canonical MIR rendering and the MIR interpreter remain unchanged;
- ABI 1.21 symbol and version negotiation are exact;
- LLVM `-O3` and MIR-interpreter retained-object checksums agree; and
- ADR 0099's compatible paired 50-sample checksum, median, nearest-rank P99,
  and peak-RSS gate passes before the fused path closes.

## Documents/components affected

Runtime and ABI architecture, native-ABI version/symbol vocabulary, LLVM
lowering plans and declarations, native allocation/access adapters, compiler
and runtime conformance tests, benchmark evidence, and implementation roadmaps.
