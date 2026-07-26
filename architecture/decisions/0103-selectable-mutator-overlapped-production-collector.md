# ADR 0103: Selectable Mutator-Overlapped Production Collector

- Status: accepted
- Date: 2026-07-26
- Depends on: ADR 0038, ADR 0039, ADR 0070, ADR 0077, ADR 0078, ADR
  0080, and ADR 0101
- Supersedes: the experimental non-selection status of the generational
  collector in ADR 0038 and the ABI 2 production-facade staging restriction in
  ADR 0070; the default ABI 1 stable-token facade remains unchanged

## Context

The generational collector already has precise moving roots, scheduler-local
nurseries, SATB marking, cards, bounded assists, lazy sweeping, task-frame root
containers, and persistent worker queues. Its worker calls nevertheless joined
each batch before returning from the safe point. That proved parallel helper
correctness, not mutator-overlapped production collection. The only linked
native facade also reported ABI 1 and forced every allocation into stable
mature space.

Production selection requires one closed composition. It cannot be inferred
from individual methods, enabled by an environment variable, or reported by
the ABI 1 archive. Mutator overlap must retain deterministic application order,
bounded queues and assists, exact root restoration, SATB/card correctness, and
fail-closed ABI negotiation.

## Decision

`GenerationalRuntime::production_with_background_workers` is the explicit
portable production constructor. Only this constructor reports
`ProductionConcurrentGenerational`.

- Mature mark and sweep jobs consume immutable sequence-numbered snapshots.
  A safe point may dispatch one bounded batch and return with work in flight.
  A later safe point joins and applies results in deterministic sequence order.
- Per-mutator bounded SATB buffers preserve overwritten references. Buffer
  draining precedes mark work, and post-scan stores shade their new targets.
- Card refinement may overlap mutator execution. Every job carries the
  reference-mutation version used to create its snapshot. A changed version
  discards the stale result and deterministically resubmits the current dirty
  cards before minor relocation.
- Lazy sweep commits only completed bounded batches. Allocation assists retain
  configured work budgets and cannot drain an unbounded heap.
- Each managed epoch publication installs an `ActiveStackWatermark` at the
  exact compiler `SafePointId`. The canonical root array is the complete
  backend-private active-root region for that poll, so processing its exact
  `RootSlot`s advances the watermark without scanning ambient host stack
  memory. Ready and suspended coroutine/task frames remain separate precise
  heap-like root containers.
- The scheduler binds one `MutatorId` and `SchedulerId` to every managed native
  operation. Writable root and task-frame restoration completes before managed
  code resumes.

The native facade has two static build compositions:

- the default build retains ADR 0070's stable-token ABI 1 facade and reports
  only ABI 1 support; and
- the `production-generational` build feature selects the production
  constructor, nursery-eligible native allocation, persistent bounded workers,
  exact ABI 2.0 support, and `ProductionConcurrentGenerational`.

The two archives are build artifacts selected before link. They do not coexist
behind runtime dispatch, an environment variable, a registry, or symbol-name
lookup. A production archive rejects ABI 1 because ABI 1 generated code cannot
reload relocated roots. A stable archive rejects ABI 2.

## Consequences

- Mature tracing, sweeping, and remembered-set refinement can overlap mutator
  execution while application remains deterministic.
- Native ABI 2 execution uses the same scheduler/task-root lifecycle as ABI 1
  while allowing nursery tokens to move.
- Stale physical tokens remain invalid; no forwarding/read-barrier fallback is
  introduced.
- Default builds remain stable-token compatible. Production selection is an
  explicit separately built archive and cannot silently change an ABI 1
  executable.
- Worker startup failure is a production composition startup failure rather
  than a fallback to a lower collector.

## Alternatives considered

### Report production from the cooperative constructor

Rejected because joining all worker work before return provides no mutator
overlap.

### Change the one process-global runtime after startup

Rejected because profile selection is a load/link contract and dynamic
collector replacement would invalidate roots, allocation contexts, and ABI
facts.

### Apply stale card-refinement results

Rejected because a mutator store after snapshot creation could hide the only
mature-to-young edge.

### Let production continue accepting ABI 1

Rejected because ABI 1 code retains stale SSA tokens after a relocating safe
point.

## Required conformance tests

- only the production constructor reports
  `ProductionConcurrentGenerational`;
- the ordinary worker constructor continues to report
  `RelocationConformance`;
- a mature batch remains in flight after a safe point returns and SATB
  preserves both snapshot and post-scan edges;
- an overlapped card mutation invalidates and restarts refinement before minor
  relocation;
- repeated deterministic overlap stress preserves the latest edge, bounded
  retries, and submitted/completed worker accounting;
- exact epoch publications expose stack-watermark safe point and processed-slot
  progress;
- the default native facade reports ABI 1 and rejects ABI 2;
- the production native facade reports only ABI 2.0, allocates in the moving
  nursery, rewrites a forced root through a scheduler-bound safe point,
  preserves its payload, and rejects the stale token; and
- task-frame, unwind, and FFI transitions restore relocated roots before
  managed execution resumes.

## Documents/components affected

Runtime/ABI and GC architecture, closed decisions, implementation roadmap,
PLRI collector profiles, collector coordination/workers/barriers, native
facade build composition and identity, LLVM profile negotiation, scheduler
root integration, conformance tests, and production stress gates.
