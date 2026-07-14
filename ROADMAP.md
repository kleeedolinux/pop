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
- [ ] Complete concurrent mature marking, SATB barriers, sweeping, pacing,
  bounded pause work, deterministic failure behavior, and stress testing.
- [ ] Stabilize the versioned PLRI and native ABI required by `0.1.0`, including
  safe points, stack maps, barriers, pin/root transitions, panic/unwind paths,
  process arguments, and standard adapters.
- [ ] Meet named correctness, throughput, memory, and latency gates on declared
  supported target profiles. Report bootstrap, relocation-conformance, and
  production collector results separately.
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
