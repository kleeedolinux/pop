# ADR 0099: Paired Native Heap Regression Gate

- Status: accepted
- Date: 2026-07-23
- Depends on: ADR 0008, ADR 0039, ADR 0070, ADR 0072, and ADR 0085
- Supersedes: none

## Context

The native stable-generational runtime has two checksum-validated managed-heap
workloads with deliberately different pressure:

- `allocationChurn` creates 20,000 short-lived 256-element scalar arrays; and
- `objectArray` retains 200,000 objects through one managed-reference array.

Optimizing either workload alone has already produced changes that were neutral
or adverse for the other. A median-only result can also hide a tail-latency or
resident-memory regression. `ROADMAP.md` therefore requires both workloads to
become one mandatory gate, but “materially worsens” did not define a
deterministic comparison contract.

Machine-local numbers are not portable performance promises. The gate must
compare compatible evidence from one declared host profile and collector stage,
validate every timed execution, and reject incomplete evidence rather than
silently ranking it.

## Decision

### Evidence contract

The benchmark result schema records, for every measured runtime/workload pair:

- the exact ordered execution-time samples as integer nanoseconds;
- the exact ordered peak-resident-memory samples in KiB;
- median and nearest-rank P99 execution time;
- maximum observed peak resident memory;
- the collector stage where applicable;
- the workload-source SHA-256 digest; and
- the SHA-256 digest of the exact expected checksum output.

Every warmup and timed execution must produce the accepted checksum. A mismatch
fails the run; the runner cannot skip that sample or emit a successful result.
Peak resident memory is mandatory for heap-gate evidence. A host that cannot
measure it fails closed for this gate while remaining usable for ordinary
non-gating benchmark exploration.

The document records a closed host profile containing target architecture,
target operating system, available parallelism, processor identity, and any
declared CPU affinity. `cpuAffinity` is `unrestricted` by default; a selected
logical CPU is recorded as its decimal identity. This prevents a hybrid host's
performance and efficiency cores from being silently mixed into one
machine-local claim. A baseline and candidate are comparable only when their
host profile, collector stage, workload source digest, sample count, and
timing/memory measurement methods match exactly.

### Paired gate

The native heap gate consumes one baseline document and one candidate document.
Both must contain `poplang` results for exactly the required heap workloads
`allocationChurn` and `objectArray`, with at least 50 measured samples per
workload and the exact accepted checksum digest.

For each workload, the gate rejects the candidate when any of these ratios is
greater than `1.05`:

```text
candidate median time / baseline median time
candidate nearest-rank P99 time / baseline nearest-rank P99 time
candidate maximum peak RSS / baseline maximum peak RSS
```

Nearest-rank P99 sorts the samples and selects rank
`ceil(0.99 * sample_count)`, using one-based ranks. The median sorts the samples
and selects index `sample_count / 2`; the required 50-sample evidence therefore
uses the upper middle sample consistently with the existing runner.
The implementation compares integer candidate evidence against
`baseline * 105 / 100` by checked integer cross multiplication; it does not
decide the boundary with binary floating-point division.

The gate is atomic: both workloads and all three budgets pass, or the heap
change fails. It reports every failed or missing condition in one structured
result. External runtime comparisons never affect acceptance.

### Scope and production follow-up

This first gate applies to `NativeStableGenerationalConformance`, because that
is the selectable native stage exercised by both workloads today. A later
accepted production collector descriptor must retain this paired gate and add
the architecture-required pause, GC CPU, memory-limit, concurrency, and
application-latency profiles. Stable-stage evidence cannot be relabeled as
production evidence.

Checked-in machine-local timing baselines are not required. CI or a release
profile may supply a baseline artifact captured on its declared host. Result
documents and gate reports are versioned machine-readable evidence; prose or a
checkbox is not a substitute.

## Consequences

- A heap optimization cannot close on one favorable median.
- Checksum, tail-time, and resident-memory regressions fail before roadmap
  status changes.
- Host-local results remain honest and cannot be compared across collector
  stages or incompatible machines.
- The 5% threshold is deliberately strict; an intentional budget revision
  requires architecture review and updated baseline evidence.
- This gate does not by itself establish production collector readiness or a
  comparative performance claim.

## Alternatives considered

### Gate only the workload being optimized

Rejected because allocation and retained-object changes share allocator,
metadata, access, and barrier paths and can regress one another.

### Use only the median

Rejected because a faster median can coexist with worse tail behavior or
substantially higher memory use.

### Treat P99 as an interpolated statistic

Rejected for the initial gate because nearest-rank selection is simple,
deterministic, and directly auditable from the recorded samples.

### Commit one development-host timing as a portable baseline

Rejected because scheduler load, hardware, operating system, and toolchain
state materially affect process-level measurements.

## Required conformance tests

- every warmup and timed execution is checksum-validated;
- result documents derive median, nearest-rank P99, maximum peak RSS, source
  digest, and checksum digest from their recorded evidence;
- the gate rejects fewer than 50 samples, missing memory evidence, a missing
  workload, a wrong runtime or collector stage, and incompatible host/source
  contracts;
- either workload fails atomically for a median, P99, or peak-RSS ratio greater
  than 1.05;
- an exact 1.05 ratio passes and a ratio above it fails without floating-point
  rounding ambiguity;
- all gate failures are reported together; and
- external runtime results cannot change native heap-gate acceptance.

## Documents/components affected

Garbage collector architecture, implementation roadmap, benchmark runner and
documentation, benchmark conformance tests, architecture-regression tests, and
`ROADMAP.md`.
