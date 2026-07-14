# Pop Lang 0.1.0 Roadmap

## Release status

- Current release candidate: `0.1.0-rc.3`
- Target release: `0.1.0`
- Goal: the first supported Pop Lang release with the complete base language,
  runtime, standard foundation, and ordinary build workflow.

This file tracks delivery. It does not define language behavior. Accepted ADRs
and the documents under [`architecture/`](architecture/README.md) remain the
source of truth. A checkbox closes only after its architecture, deterministic
tests, implementation, documentation, and applicable cross-backend evidence
agree.

## Included in 0.1.0-rc.3

The release candidate already establishes the backend-neutral compiler pipeline
and executable coverage for:

- typed strings, escapes, concatenation, interpolation, and primitive
  formatting;
- numeric ranges, `break`, `continue`, decimal floats, complete ordering,
  casts, and checked numeric conversions;
- fixed arrays and typed tables with indexing, mutation, and deterministic
  table growth;
- tuples, destructuring, fixed multiple returns, and exact multiple assignment;
- runtime constants, erased type aliases, and nominal scalar enums;
- explicit callable generics, generic records, and generic tagged unions through
  deterministic concrete MIR specialization;
- records, tagged-union matching, native classes, nominal interfaces, closures,
  compile-time evaluation, the MIR interpreter, and LLVM native execution.

The C11 transpiler remains an isolated experiment. It is not a release backend
and does not define the work remaining for `0.1.0`.

## Release blockers

### 1. Complete the base language

- [x] Implement optional flow narrowing, pattern binding, defaulting, and
  propagation operators without weakening static typing.
- [x] Implement the complete typed-error workflow: declarations, `Result`,
  propagation, matching boundaries, explicit MIR failure and cleanup
  edges, diagnostics, and checked XML documentation.
- [x] Finish generalized `for` over the nominal `Iterable<T>` and `Iterator<T>`
  protocols, including deterministic `Sequence` adapters. Keep arrays
  fixed-length; provide growth through the accepted growable collection type.
  - [x] Accept ADR 0053 and implement reserved `Iterable<T>`, `Iterator<T>`, and
    `Iteration<T>` identities with exact static protocol calls.
  - [x] Execute array, table, `List<T>`, and nominal iterator traversal through
    the MIR interpreter and LLVM with deterministic order.
  - [x] Implement ordinary lazy `Sequence.map`/`filter`, eager `fold`, and
    materializing `collect` in `Pop.Standard` Pop source.
  - [x] Reject statically proven structural mutation of the directly iterated
    collection while preserving typed indexed replacement.
  - [x] Accept ADR 0056 to close first-class `Range<TInteger>` construction,
    typing, cost, iteration, and backend contracts without a range operator.
  - [x] Implement `Range.create` and its interpreter/LLVM generalized-iteration
    conformance, including zero-step and checked-advancement behavior.
- [x] Complete generic behavior needed across Bubble boundaries: portable
  reference metadata, type-argument inference, constraints, and the accepted
  typed sharing/specialization policy. Preserve full specialization as a valid
  bootstrap strategy.
  - [x] Accept ADR 0054 and implement complete call-site inference with exact
    nominal bounds and deterministic failure diagnostics.
  - [x] Specialize generic classes, interfaces, fields, methods, and exact
    witnesses without erased or dynamic fallback.
  - [x] Emit and consume logical portable specialization capsules containing
    private transitive helpers without widening Bubble visibility.
  - [x] Execute cross-Bubble generic capsules through the MIR interpreter and
    LLVM, including ordinary `Sequence` implementations.
- [ ] Complete coroutines, async functions, awaiting, cancellation, and scoped
  cleanup with one backend-neutral HIR/MIR contract.
- [ ] Close the remaining accepted first-release gaps for FFI, view lifetimes,
  checked casts, effects, and generated typed metadata adapters before exposing
  those surfaces as stable.

### 2. Finish the standard foundation

This blocker closes the exact ADR 0058 bootstrap foundation. It does not promote
the phase 1+ catalog to implemented status. Async/task execution remains a base-
language and runtime blocker, while source-free dependency selection and native
linking remain ordinary-workflow blockers in section 4.

- [x] Freeze the exact `Pop.Standard` prelude, public root inventory, stable
  identities, tier/status metadata, and API baseline.
  - [x] Accept ADR 0058 and freeze the exact primitive, foundation, protocol,
    task/cancellation, trusted-attribute, typed-output, and `Sequence` prelude
    bindings without adding a nominal `Option<T>` beside `T?`.
  - [x] Add a versioned canonical API baseline with append-only identities,
    tier/status boundaries, bootstrap cross-checks, and fail-closed loading.
  - [x] Bound baseline size and reject noncanonical identity, namespace,
    prelude-tier, and documentation-authority fields before resolution.
  - [x] Resolve `Sequence` as the sole trusted low-priority prelude namespace
    root while preserving nearer declarations and explicit aliases.
- [x] Make the exact optional `T?`, `Result`, essential collection, iteration,
  `Sequence`, and `String` foundation executable, and publish the reserved byte,
  numeric-protocol, resource, and task/cancellation identities without
  presenting their planned catalog APIs as implemented.
  - [x] Make optional `T?` values and the reserved `Result<T, TError>` workflow
    usable without a dynamic carrier or duplicate nominal `Option<T>` wrapper.
  - [x] Execute fixed arrays, typed tables, growable `List<T>`, integer ranges,
    nominal iteration, and portable `Sequence` algorithms through the MIR
    interpreter and LLVM.
  - [x] Execute immutable UTF-8 `String` literals, concatenation,
    interpolation, closed primitive formatting, and value equality through the
    shared runtime contract.
  - [x] Keep `Bytes`, `Equal`, `Order`, `Hash`, `Close`, `AsyncClose`, `Task`,
    and `CancelToken` at their exact reserved type/status boundary. Byte/text
    views, the `Math` API, resource operations, and task execution advance only
    through their separately accepted language, runtime, and catalog slices.
- [x] Keep every portable callable in the frozen API baseline in ordinary
  `.pop` Modules so adding an algorithm does not require compiler or backend
  changes.
  - [x] Move deterministic `Sequence` adapters into the conventionally
    discovered `Pop.Standard` `sequence.pop` Module.
- [x] Emit, verify, and round-trip load deterministic `.poplib` artifacts
  containing manifests, public reference metadata, checked documentation,
  target implementations, hashes, ABI requirements, and exact Bubble
  dependencies.
  - [x] Implement the verified logical public reference-metadata model and
    source-free cross-Bubble consumption path.
  - [x] Accept ADR 0055 for canonical JSON control files, SHA-256 inventories,
    bounded loading, and versioned lock/artifact/capsule schemas.
  - [x] Round-trip canonical public reference metadata with SHA-256-verified
    portable generic HIR/type capsules and source-free specialization.
  - [x] Emit and load bounded `.poplib` directories with canonical manifests,
    checked file inventories, documentation, and opaque target implementations;
    reject malformed, missing, extra, traversal, and corrupted content.
  - [x] Emit deterministic schema-versioned `documentation.xml` from checked
    symbol-owned XML fragments.
  - [x] Make `pop build` emit and immediately verify each discovered library
    Bubble's `.poplib` with exact identity, source/API hashes, dependencies,
    checked documentation, and the selected native target implementation.
- [x] Complete checked public XML documentation, compiled examples, allocation
  and cost notes, and interpreter/LLVM differential tests for every portable
  callable in the frozen foundation baseline.
  - [x] Preserve checked public documentation by stable `SymbolIdentity` and
    reject duplicate output member IDs deterministically.
  - [x] Complete checked type-parameter, iteration, allocation, complexity, and
    dispatch documentation plus compiled examples for the portable `Sequence`
    baseline.
  - [x] Keep the native `print` overloads and portable `Sequence` callables
    labeled `prototype`; no callable advances to `implemented` or `stable`
    without the complete ADR 0058 evidence gate.
  - [x] Synchronize the accepted `Actor` and `Cluster` roots across the
    canonical public inventory, owning system/network catalog, implementation
    phase, and architecture conformance snapshot without adding either root to
    the frozen prelude or implemented API baseline.

The broad catalog after the standard foundation remains planned work. It is not
necessary to implement every format, network, media, data, tooling, or AI
Package for the first release.

Post-baseline library work has begun without widening the release foundation:

- [x] Append the first `Sequence` terminal, inspection, visitation, bounded
  lazy, composition, and checked integer aggregate prototypes to the API
  baseline with interpreter/LLVM differential coverage.
- [x] Replace the Rust-only Math prototype with ordinary portable Pop `Int`
  functions and checked overflow behavior.
- [x] Add an exact machine-readable public-root/tier/status projection without
  presenting planned catalog families as implemented.
- Define view lifetimes before exposing `Bytes` views or `Text.View`.
- Make reserved `Iteration<T>` exhaustively matchable in ordinary source
  before adding no-fallback sequence inspection.
- Complete LLVM aggregate representation for collections whose element is
  optional; MIR already preserves the typed optional item contract.

### 3. Make the runtime release-ready

- [ ] Replace bootstrap-only stable handles with the accepted production
  generational path: a real moving nursery, typed root/edge relocation,
  remembered cards, promotion, and backend capability negotiation.
  - [x] Implement the single-mutator moving nursery, exact root/edge/handle/pin
    relocation, remembered cards, deterministic promotion, and page-described
    allocation conformance path.
  - [x] Keep ownership separate from placement/generation/pinning, publish
    complete scheduler-local graphs explicitly into shared ownership/pages, and
    reject shared-to-local edges before mutation.
  - [x] Add exact-one-owner isolated-region construction, distinct placement and
    accounting, zero-copy scheduler transfer, protected owner capabilities,
    external-edge/root/pin rejection, and explicit dissolution.
  - [x] Add scheduler-indexed object/page ownership, independent TLAB cursors,
    per-scheduler minor requests and evacuation scope, and cross-scheduler local
    edge rejection.
  - [x] Add scheduler-owned scoped bump arenas with disjoint typed managed/arena
    slots, precise relocating managed roots, same-arena edge enforcement,
    hard-limit accounting, stale-token checks, and bulk reclamation.
  - [x] Count scoped pin handles and uniquely pinned objects, profile active and
    completed lifetime in deterministic safe-point units, report long-lived
    pins once, and keep first/additional pin transitions independent of heap
    size.
  - [ ] Add shared immutability proofs and capability-driven barrier elimination.
  - [ ] Complete production backend writable-root capability negotiation and
    parallel scheduler-local allocation/evacuation.
- [ ] Complete concurrent mature marking, SATB barriers, sweeping, pacing,
  bounded pause work, deterministic failure behavior, and stress testing.
  - [x] Implement cooperative incremental SATB marking/sweeping with bounded
    slices, ordered lazy sweep discovery with no full-heap transition inventory,
    and correct late-root, allocation, and overwritten-edge handling.
  - [x] Enforce byte admission before mutation, adaptive targets, protected
    emergency/evacuation reserves, typed non-heap accounting, bounded allocation
    assists, empty-page return, deterministic OOM, and pressure/debt/domain
    telemetry.
  - [x] Add typed mutator registration and bounded epoch handshakes with exact
    once-only acknowledgements, published root/TLAB/barrier state, explicit
    foreign-execution states, and deterministic transition telemetry.
  - [x] Integrate the marking epoch with the generational runtime: registered
    mutators now gate major-cycle activation until every precise root snapshot
    is validated and acknowledged, no worker work is dispatched before that
    boundary, and nursery relocation remains deferred while snapshots retain
    physical tokens.
  - [x] Replace native `BootstrapRuntime` composition with the accepted
    stable-token generational stage so real ABI 1 executables use mature SATB
    marking, bounded sweeping, pacing, and page allocation without prematurely
    enabling nursery relocation or evacuation.
  - [x] Batch empty-page inventory reclamation once per mature sweep, index the
    active mature page by exact layout and scheduler, initialize scalar arrays
    in one pass, and scalar-replace non-escaping read-only loop-local arrays
    while preserving traps, safe points, and managed-path negatives.
  - [x] Profile the real retained managed-object workload and remove its
    bootstrap-era allocation costs: ABI 1.11 now publishes initialized objects
    atomically, committed-byte accounting is constant-time, stable mature
    stores skip impossible nursery cards, managed arrays initialize before
    publication, active mature spans retain mutator-local cursors, small
    payloads remain inline as untagged one-word slots interpreted by precise
    maps, homogeneous managed arrays classify stores in constant time, and
    object/placement metadata uses deterministic arena-indexed token segments
    without duplicate entry tokens. On the development host this reduced the
    checksum-validated `objectArray` median from about 408 ms to 34.223 ms in a
    50-sample run (Go: 4.896 ms); the immediately preceding retained-heap slice
    measured about 38.0 ms. These are host-local optimization results;
    direct page-backed payload access and inline conditional barriers remain
    required before the production throughput gate can close.
  - [x] Add opt-in persistent host workers with bounded owner-FIFO queues,
    opposite-end peer stealing, parallel exact object-map and collecting-safe-
    point remembered-card scans, deterministic result application, sweep
    dispatch, worker/steal telemetry, and joined shutdown.
  - [x] Divide large pointer, mixed-layout, and large pinned scans into bounded
    precise-slot chunks with one continuation per object, skip field tracing for
    pointer-free large objects, and preserve SATB/post-scan mutation barriers in
    cooperative and worker modes.
  - [x] Group pages into domain- and scheduler-homogeneous regions, expose exact
    live/committed/fragmentation/pin/reference telemetry, drive shared-region
    mark/sweep states, and select only bounded positive-benefit evacuation sets
    that fit the protected reserve while excluding pinned and large regions.
  - [x] Evacuate selected shared regions through a failure-atomic stopped-mutator
    slice that copies objects into compact monomorphic pages, rewrites precise
    fields, stack roots, strong handles, and card metadata, invalidates old
    tokens, quarantines retired regions before removal, and accounts peak use of
    the protected evacuation reserve.
  - [x] Attach the persistent bounded worker pool to an already configured
    runtime, stage selected-object evacuation copies on the collector, dispatch
    their internal-edge rewrites across workers, restore deterministic result
    order, and leave the final external-edge/root/placement commit
    collector-owned and atomic.
    Phase-specific resolution and mutator-concurrent evacuation remain
    production work.
  - [ ] Complete native scheduler/runtime transition integration, then add
    adaptive worker sizing and stealing policy, concurrent card refinement and
    page reclamation, stack watermarks, race/stress proof, and latency
    measurements.
    - [x] Add the bounded synchronized M:N correctness scheduler, deterministic
      record/replay, explicit migration refusal, isolated bounded blocking
      workers, host/virtual timers, external events, failure containment, and
      initial wake/cancellation/migration stress coverage.
    - [x] Add bounded enabled-set schedule exploration and the versioned,
      checksum-validated synchronized-reference benchmark for task control,
      ready polls, injection, hot-queue stealing, suspended frames, timers,
      external events, and blocking saturation.
    - [x] Add exact current/high-water queue and blocking depth, bounded steal
      search/outcome/batch, and worker lifecycle telemetry, including a final
      shutdown snapshot and the `pop-scheduler-benchmark-v2` schema.
    - [ ] Bind scheduler transition events to native mutator registration,
      precise suspended-frame root publication, collector epochs, and the ABI 2
      writable-root transition before claiming production GC integration.
      ADR 0072 requires the stronger ready-and-suspended task-frame root
      lifecycle rather than rooting only explicit suspension.
      - [x] Move canonical `SchedulerId` ownership into PLRI and add typed,
        bounded retained task-root container identities.
      - [ ] Retain exact initial/nonterminal frame roots before queue
        publication and restore relocated `RootSlot` values before dispatch.
      - [ ] Register each normal worker as a detached mutator, bind managed
        native entries per dispatch, and make safe points acknowledge epochs
        exactly once.
      - [ ] Prove scheduler-local allocation ownership, root-container
        migration/refusal, and exact cleanup under forced minor/major GC.
    - [ ] Extend declared benchmark profiles with local/foreign wake and
      ping-pong latency, continuous I/O fairness, steal-storm and million-frame
      memory evidence, operating-system resource counters, and scheduler/GC
      interaction after the production collector binding exists.
    - [ ] Add work-budget exhaustion, event/timer poll and delivery-delay,
      blocking shutdown-delay, ready-to-run percentile, and scheduler migration
      telemetry before treating observability as complete.
- [ ] Stabilize the versioned PLRI and native ABI required by `0.1.0`, including
  safe points, stack maps, barriers, pin/root transitions, panic/unwind paths,
  process arguments, and standard adapters.
- [ ] Meet named correctness, throughput, memory, and latency gates on declared
  supported target profiles. Report bootstrap, relocation-conformance, and
  production collector results separately.
  - [x] Establish checksum-validated host workloads for 20,000 short-lived
    256-element arrays and a retained 200,000-object array, then record the
    first 15-sample Pop/Go execution-only baseline on the local i5-1235U host.
    Current medians are 44.468 ms versus 4.715 ms for allocation churn and
    139.161 ms versus 4.828 ms for the retained object array; these are local
    optimization evidence, not portable performance claims.
  - [x] Re-run the same host-only workloads after the stopped-mutator
    evacuation worker slice. The 15-sample medians were 48.102 ms versus
    5.640 ms for allocation churn and 139.667 ms versus 4.899 ms for the
    retained object array. These bootstrap workloads do not exercise selective
    shared-region evacuation, so the result is a sequencing checkpoint rather
    than evacuation evidence.
  - [x] Reject speculative bootstrap access-probe coalescing after an
    interleaved 41-sample retained-object A/B measured 139.356 ms for the
    candidate versus 137.202 ms for the committed baseline. The next retained
    array optimization must target the native ABI/storage boundary and preserve
    precise managed barriers instead of repeating this local refactor.
  - [x] Remove root-publication allocation from bootstrap safe points when no
    collection is pending, and materialize bulk-initialized arrays in one pass.
    Same-binary A/B runs show about 2% improvement from one-pass initialization;
    safepoint allocation removal is neutral for zero-root churn and about 3%
    better for the retained-root workload under noisy 25-run host sampling.
  - [ ] Reduce repeated native ABI locking/handle lookups for verified managed
    array and field access. One-word precise payload slots, constant-time
    homogeneous-array classification, and token-derived segmented directory
    entries reduced the checksum-validated 50-sample retained-object median
    from about 38.0 ms to 34.223 ms (Go: 4.896 ms) on the development host.
    Direct page-backed access and removal of the process-global common-path
    mutex remain open; repeat both workloads and add production-collector
    throughput/tail-latency gates once selectable.

#### Remaining heap problems

- [ ] **Heap changes can optimize one workload while regressing another.**
  `allocationChurn` and `objectArray` are not yet one mandatory performance
  gate. Fix this by rejecting any heap change that regresses either 50-sample
  median by more than 5%, breaks its checksum, or materially worsens P99 or
  peak memory.
- [ ] **Logical pages do not own physical object payloads.** Objects still keep
  separate host allocations and duplicate object/placement metadata, so page
  locality does not accelerate ordinary reads, writes, or scanning. Fix this
  with monomorphic page-backed payloads, page-shared layouts, compact side
  metadata, and token-derived placement.
- [ ] **The native common path is globally serialized.** Allocation, array
  access, field access, and barriers repeatedly lock the process-global runtime
  and look up opaque tokens. Fix this with scheduler/thread-local active pages,
  TLAB cursors, barrier buffers, and checked direct access; keep global work on
  explicit refill, publication, safe-point, and collection slow paths.
- [ ] **Allocation rebuilds layout information at runtime.** Pointer maps are
  cloned, sorted, and searched even though the compiler already knows each
  allocation layout. Fix this with compiler-emitted static layout and
  allocation-site descriptors shared by allocation, access, barriers, and
  tracing.
- [ ] **Reference stores still pay too much barrier machinery.** The runtime
  reaches generic SATB/generational logic for cases that can be classified
  cheaply. Fix this with inline conditional barriers, per-mutator buffers,
  range barriers, unpublished-initialization elision, and adaptive pretenuring
  for allocation sites with sustained high survival.
- [ ] **ABI 1 prevents native nursery relocation.** LLVM roots are published
  but cannot be rewritten and reloaded across every control-flow path, forcing
  native allocations into stable mature space. Fix this with ABI 2 writable
  roots, forced-relocation tests, stale-token rejection, and verified reloads
  for stacks, registers, coroutines, unwind paths, and FFI transitions.
- [ ] **The production concurrent collector is not selectable.** Native
  scheduler integration, concurrent mature work, card refinement, lazy sweep,
  stack watermarks, bounded assists, and race/stress proof remain incomplete.
  Fix all of them before reporting `ProductionConcurrentGenerational`.
- [ ] **Retained-object throughput is still far from the target.** The current
  local result is about 34 ms. Fix the page/access bottlenecks without a churn
  regression, reach below 25 ms first and below 12 ms next, then compare Pop
  against other runtimes only when P99, GC CPU, memory, and pause budgets also
  pass.

- [ ] Prove representative programs behave the same through canonical MIR, the
  MIR interpreter, optimized MIR, and LLVM native execution.

### 4. Complete the ordinary user workflow

- [ ] Finish deterministic Package, Bubble, and Workspace discovery;
  `bubble.toml`; one `bubble.lock`; dependency resolution; features; target
  selection; and reproducible caching.
  - [x] Parse canonical Package manifests, structured registry/local/exact-Git
    requirements, development/platform scopes, and combined Package/Workspace
    roots.
  - [x] Discover conventional Bubbles and restricted Workspace members with
    non-overlapping ownership and deterministic default-member selection.
  - [x] Resolve, cycle-check, analyze, and link exact local-path Package
    dependencies through public Bubble metadata.
  - [x] Generate one canonical SHA-256-backed `bubble.lock` for selected local
    Package graphs, round-trip it fail-closed, write it atomically, and enforce
    `--locked`, `--offline`, and `--frozen` update policy.
  - [x] Use one shared Workspace `target/` root without widening visibility.
- [ ] Make the supported `pop check`, `pop build`, `pop run`, `pop test`,
  `pop documentation`, `pop format`, `pop lint`, and `pop fix` workflows operate
  on real Packages and Workspaces with structured machine output.
  - [x] Make `pop check`, `pop build`, and `pop run` operate on manifest-selected
    Packages and virtual Workspace default members.
  - [x] Make `pop documentation` emit checked deterministic public XML for
    selected library Bubbles.
- [ ] Complete deterministic native linking, test/example/benchmark Bubbles,
  public reference loading, initialization order, and clear capability errors.
  - [x] Link Package library and binary Bubbles by exact public
    `SymbolIdentity`, including generic consumer specialization.
- [ ] Implement stable structured diagnostics with `POP####` codes, warning
  policy, semantic fixes, atomic safe fix-all, JSON/LSP rendering, and bounded
  error recovery.
- [ ] Accept or replace the still-proposed toolchain distribution design before
  shipping installers, update metadata, signing, or self-update behavior.

### 5. Pass the 0.1.0 release gates

- [ ] Resolve every architecture gap affecting a shipped feature through an
  accepted ADR before stabilization.
- [ ] Pass formatting, unit, conformance, integration, architecture-regression,
  documentation, interpreter/LLVM differential, GC stress, and supported-target
  tests in CI.
- [ ] Add parser and IR-verifier fuzzing, malformed-input/resource-limit corpora,
  and permanent regressions for static typing, visibility, compile-time
  capability, backend neutrality, and Lua regressions.
- [ ] Verify all architecture links, canonical examples, naming, visibility,
  public API snapshots, artifact reproducibility, and dependency boundaries.
- [ ] Declare the supported operating systems and architectures, publish known
  limitations, freeze the `0.1.0` artifact/ABI versions, and produce release
  notes from the final conformance matrix.

## Explicitly after 0.1.0

- expanding the experimental C backend beyond its accepted fail-closed subset;
- a custom VM or stable serialized MIR/bytecode compatibility promise;
- the complete official extension and public-library catalog;
- finalizers, weak references, unrestricted runtime reflection, and Bubble
  unloading;
- optimizations or platform APIs without measured and accepted contracts.

## Release rule

`0.1.0` is ready when every blocker above that applies to its accepted surface
is complete, no shipped behavior relies on an unresolved design question, and
the supported interpreter and LLVM paths satisfy the same static semantic
contract. Schedule pressure does not turn an experimental or bootstrap-only
path into a stable promise.
