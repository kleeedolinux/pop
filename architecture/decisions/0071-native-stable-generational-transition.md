# ADR 0071: Native Stable-Token Generational Transition

- Status: accepted
- Date: 2026-07-14
- Depends on: ADR 0008, ADR 0022, ADR 0038, and ADR 0039
- Supersedes: the native-bootstrap implementation staging in ADR 0039; it does
  not supersede the ABI 2 writable-root requirement

## Context

The native facade still owns a process-global `BootstrapRuntime`. Consequently,
ordinary `pop run` executables and the Pop Lang benchmark workloads cannot use
the implemented page allocator, incremental SATB mature tracing, bounded
sweeping, pacing, workers, or memory controller. Collector-only benchmarks do
not measure this executable path.

LLVM ABI 1.10 publishes precise roots but does not reload changed managed tokens
after a safe point. ADR 0039 therefore correctly forbids selecting the moving
nursery, evacuation, or the `ProductionGenerational` profile for native code.
That restriction does not require retaining the bootstrap mark/sweep
implementation for allocations whose physical tokens are guaranteed stable.

## Decision

The native facade replaces its process-global `BootstrapRuntime` with a closed
stable-token composition over the generational collector.

- Every ABI 1 native allocation is placed directly in a non-moving mature,
  large, or pinned domain. `NurseryEligible` is conservatively placed as mature.
- Native safe points use incremental SATB mature marking and bounded sweeping.
- Native ABI 1.10 root slots remain read-only because this composition never
  relocates their managed tokens.
- Native execution does not select evacuation and never reuses a stale moved
  token through forwarding, a read barrier, or a stable-handle fallback.
- The collector reports the distinct
  `NativeStableGenerationalConformance` stage. It is neither the removed
  bootstrap collector nor `ProductionConcurrentGenerational`.
- Benchmark records must name this stage. Results from the old bootstrap path,
  relocation conformance, this native stable-token stage, and future ABI 2
  production execution are not interchangeable.

The production requirement remains unchanged: LLVM may enable moving nursery
allocation and evacuation only with ABI major 2 after emitted-code inspection
and forced native relocation tests prove that every live managed value is
reloaded across all control-flow paths.

No runtime registry or environment-selected collector is introduced. The
native archive has one statically composed collector and calls it directly.

## Consequences

- Real Pop Lang executables exercise the new mature collector and allocator.
- Allocation-churn measurements can drive native allocator, pacing, and sweep
  optimization instead of measuring the deleted bootstrap heap.
- ABI 1 remains honest and compatible because native managed tokens do not move.
- The moving nursery and selective evacuation remain unavailable to native LLVM
  until the accepted writable-root proof is complete.
- The portable bootstrap collector may remain as isolated conformance and
  comparison infrastructure, but the native facade no longer depends on it.

## Alternatives considered

### Select the moving collector under ABI 1 for rootless benchmarks

Rejected because a workload-specific accident cannot authorize a backend
capability. The same archive must remain correct for every valid program.

### Preserve forwarding aliases for stale LLVM values

Rejected because ADR 0039 requires evacuated tokens to become invalid and the
GC architecture excludes a production read-barrier fallback.

### Keep benchmarking the bootstrap collector until ABI 2 is complete

Rejected because it hides mature allocator and reclamation costs from the real
native workload and gives no optimization signal for the implemented collector.

## Required conformance tests

- native state contains the stable-token generational composition and no
  `BootstrapRuntime`;
- native allocation, access, roots, pins, strings, tables, lists, ranges, and
  iteration retain their ABI 1.10 behavior;
- forced native collection reclaims unreachable mature allocations while roots
  and pins preserve reachable objects;
- every native allocation placement is non-moving under ABI 1;
- native identity reports `NativeStableGenerationalConformance` and does not
  report production;
- collector access and table growth preserve precise object maps and barriers;
- architecture regression tests reject reintroducing bootstrap native state or
  enabling native evacuation before ABI 2 root reload proof;
- the Pop Lang allocation-churn workload is checksum-validated before timing.

## Documents/components affected

Runtime and ABI architecture, GC staging, implementation roadmaps, PLRI
collector-stage vocabulary, portable collector composition, native facade,
native ABI tests, architecture conformance tests, and Pop Lang benchmark
metadata.
