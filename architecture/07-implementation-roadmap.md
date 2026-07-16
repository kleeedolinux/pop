# Implementation Roadmap

The roadmap validates architecture in vertical slices. Each milestone ends with
an executable or inspectable artifact rather than a large isolated subsystem.

## Milestone 0 — Decisions and skeleton

- implement the accepted Rust 2024 virtual Cargo workspace and crate boundaries
  from ADR 0018;
- define source spans, IDs, diagnostics, and deterministic test conventions;
- derive and document a minimal Luau-first syntax subset;
- encode the accepted numeric, error, inheritance, generic, naming, Bubble, and
  GC decisions as compiler contracts/tests;
- define a small target-independent `TargetSpec`.
- define the diagnostic catalog/code ranges and typed diagnostic constructors;
- define bootstrap schemas for `Pop.Internal` primitive/intrinsic declarations.
- define documentation token attachment, safe XML subset, and initial catalog
  diagnostics.

Exit criterion: one source file can be tokenized and parsed with snapshot-tested
diagnostics.

## Milestone 1 — Front end and typed HIR

- lossless syntax tree and parser recovery;
- namespaces, using directives, explicit declaration visibility, Bubble
  reference metadata, and symbol
  resolution;
- primitive types, functions, locals, branches, `while`/`repeat` loops, numeric
  `for` ranges and loop control, tuples, immutable records,
  tagged unions, and typed `with` updates;
- decimal floating-point literals, complete numeric ordering, and explicit
  checked numeric conversions from ADR 0040;
- typed string concatenation, interpolation, escape decoding, and closed
  primitive formatting from ADR 0041;
- conditional expressions and `elseif` statement chains from ADR 0043;
- typed compound assignment with single-evaluation targets from ADR 0044;
- fixed type packs, comma returns, and exact multiple assignment from ADR 0045;
- optional comparison narrowing, `if local`/`while local` binding, lazy `??`,
  and optional-only postfix `?` from ADR 0051;
- constraint-based local inference with no dynamic fallback;
- typed UDA declarations, attachment, constant arguments, and query API;
- deterministic compile-time constant/function evaluation with budgets;
- typed HIR construction and deterministic `pop check <source.pop> --dump hir`
  bootstrap inspection;
- structured type/resolution diagnostics with initial safe quick fixes;
- parser/resolver tests for mandatory namespace visibility, same-Bubble
  `internal`, file-scoped `private`, and rejected `export` syntax;
- compile/load verified `Pop.Internal` reference metadata;
- bootstrap the `Pop.Standard` prelude and core protocols.
- parse `bubble.toml`, discover conventional Bubbles, and emit deterministic
  Workspace/Package/Bubble metadata.
- checked `<summary>`/parameter/return/`cref` documentation plus LSP hover;
- compiler-backed LSP diagnostics, related labels, quick fixes, document
  symbols, direct-call parameter inlay hints, and dependency-free same-Bubble
  snapshots;
- validated canonical binary/library scaffolding through `pop new` and
  `pop initialize`.

Exit criterion: multi-module programs type-check; UDA/compile-time tests are
reproducible; HIR contains no unresolved names, dynamic operations, or implicit
conversions.

## Milestone 2 — MIR and interpreter

- CFG/block-argument MIR;
- HIR lowering with explicit evaluation order;
- MIR parser, printer, verifier, and deterministic `pop check <source.pop>
  --dump mir` bootstrap inspection;
- portable constant folding and dead-code elimination;
- stable allocation-site/lifetime/region identities, closed call-retention
  summaries, and negative proof-verifier fixtures from ADR 0085;
- a simple MIR interpreter and minimal runtime adapter.
- warning-wave policy, scoped suppression, LSP/JSON output, and fix-all engine.
- `.poplib` `documentation.xml`, `pop documentation`, and compiled documentation examples.

Exit criterion: core language tests execute through MIR without LLVM.

## Milestone 3 — Native classes and collections

- native class fields, constructors, and direct methods;
- nominal interfaces and explicit implementation;
- optimized record layout, arrays, and statically typed tables;
- exhaustive tagged-union matching and missing-case quick fixes;
- closure conversion and captured variables;
- allocation, precise stack/object maps, and bootstrap stop-the-world GC.
- initial `Pop.Standard` collections, text, result, and iteration conformance;
- modular base-library source and focused test ownership under ADR 0035, so
  ordinary API-family work stays outside compiler and backend crates;
- conventional reserved source-root discovery and verified HIR/MIR contribution
  probes before `.poplib` emission/loading completes the source-library build.

Exit criterion: tests prove that normal class fields use resolved member access,
not table or runtime-name lookup.

## Milestone 4 — LLVM native backend

- target layout and Inkwell-confined LLVM lowering through backend-private IR;
- deterministic verified `pop check <source.pop> --dump ll` inspection;
- PLRI native ABI and runtime library;
- modular runtime ownership from ADR 0038: pure PLRI, reusable collector,
  native-ABI vocabulary, and thin native facade;
- `.poplib` Bubble manifests/reference metadata, object emission, and platform linking;
- standalone bootstrap `pop build`/`pop run` examples that exercise Rust
  `Pop.Standard` output, canonical process arguments, and allocating
  Rust-runtime operations;
- `BubbleContext` default loading and initialization;
- moving nursery, card barriers, and GC stress tests;
- bounded large-object pointer scanning with pointer-free field-scan elision;
- bounded worker-local FIFO queues with opposite-end peer stealing,
  deterministic result application, steal telemetry, and joined shutdown;
- runtime-owned major-mark epoch activation that validates and acknowledges
  every registered mutator root snapshot before tracing or worker dispatch;
- accepted native stable-token generational composition that removes the
  bootstrap collector from ABI 1 executables while reserving moving nursery and
  evacuation for verified ABI 2 writable roots;
- ABI 1.11 atomic initialized-object publication plus profiled retained-graph
  fast paths: cached committed accounting, prepublication managed-array fill,
  stable-stage barrier specialization, mutator-local mature-span cursors,
  inline one-word payload slots, constant-time homogeneous-array
  classification, and deterministic arena-indexed token metadata without
  duplicate per-entry tokens;
- backend-private scalar replacement for non-escaping scalar arrays, including
  read-only loop-local instances, with exact shape/bounds traps, loop safe
  points, and managed, escaping, or mutated-loop negative coverage;
- migrate that scalar-array escape decision into verified portable MIR, add
  every-exit activation-owned storage, and require interpreter/LLVM agreement
  before generalizing proof-directed static reclamation;
- scoped pin handle/object counting and deterministic long-lived-pin telemetry;
- domain/scheduler-homogeneous regions with exact fragmentation telemetry and
  bounded reserve-admitted selective-evacuation candidate selection;
- failure-atomic stopped-mutator selective evacuation with compact monomorphic
  destination pages, exact field/root/handle/card rewriting, stale-token
  invalidation, transient quarantine, and peak reserve admission;
- bounded worker-assisted selected-object internal-edge rewriting with
  deterministic results and one collector-owned atomic reference/placement
  commit;
- mutable typed root updates, runtime-profile/backend capability negotiation,
  and a real single-mutator relocation conformance collector before production
  TLAB/parallel-evacuation claims (ADR 0039);
- atomic shared-graph freezing with mutability kept separate from ownership,
  plus verified backend-neutral `UnpublishedOwner` barrier proofs (ADR 0080);
- `Pop.Standard` I/O, time, tasks, and platform adapters;
- debug locations and stack traces;
- differential tests against the MIR interpreter.

Exit criterion: representative multi-module programs produce native executables
whose behavior matches the interpreter.

Alongside this milestone,
[ADR 0059](./decisions/0059-experimental-secure-c-transpilation-backend.md)
authorizes an isolated experimental C11 source backend. Its first runtime-free
slice consumes optimized verified MIR, supports scalar control flow, direct
calls, and typed integer/literal-string output, preserves checked numeric
semantics without C undefined behavior, and is invoked through `pop transpile
<source.pop> --to c`. It is not a replacement for LLVM or a runtime milestone.

## Milestone 5 — Language depth

- inferred nominally constrained generics, generic classes/interfaces, and
  portable generic reference metadata under the accepted full-specialization
  correctness path; typed code sharing remains an optional verified
  optimization;
- reserved `Result`, nominal error declarations, exact `try` propagation,
  exhaustive recovery boundaries, and deterministic lexical cleanup;
- explicit checked interface-to-named-class casts with exact/descendant stable
  identity, typed optional absence, and MIR-interpreter/LLVM differential
  behavior under ADR 0095;
- coroutines/async model;
- the statically bound native FFI from ADR 0081 and ADR 0082, including exact
  C/system ABI types, typed native-link manifests/artifacts, HIR/MIR foreign
  transitions, LLVM calls, read-only byte pins, owned ABI buffers,
  generation-checked handles, callbacks, generated bindings, and ordinary safe
  wrappers;
- opt-in retained metadata and generated typed adapters where justified;
- production concurrent mature GC and latency/benchmark gates, building on the
  implemented cooperative SATB marking and ordered lazy sweeping without a
  full-heap transition inventory, page/TLAB allocation, hard-limit
  accounting, adaptive pacing, bounded assists, logical memory telemetry, the
  runtime-integrated typed bounded mark-epoch coordinator, and opt-in bounded
  host-worker mark/card/sweep dispatch; the runtime also has distinct ownership metadata,
  whole-graph local-to-shared publication, ownership barrier enforcement, and
  exact-one-owner isolated-region construction/transfer/dissolution, while
  scheduler-indexed TLABs/local collection now preserve heap independence;
  shared regions now expose exact fragmentation/pin/reference accounting,
  deterministic reserve-bounded evacuation-set selection, and a
  failure-atomic stopped-mutator relocation slice that copies into compact
  monomorphic pages, rewrites precise fields/roots/handles/card metadata,
  invalidates old tokens, and retires quarantined regions; the collector can
  stage selected-object copies for bounded workers to rewrite their internal
  edges before the deterministic collector-owned commit, while phase-specific
  reference resolution and mutator-concurrent evacuation remain open;
  typed scoped bump arenas provide precise external roots and bulk reclamation;
  proof-directed `Elided`, `StaticSlot`, and compiler-inferred `ScopedRegion`
  plans cover proven scalars, arrays, aggregates, and closure environments,
  with conservative managed fallback and exact cleanup/unwind/cancellation/
  coroutine frontiers under ADR 0085;
  fixed scheduler-local contexts now carry explicit identities, disjoint token
  namespaces, independently locked TLABs, and concurrent scheduler-scoped minor
  evacuation; replacing the ABI 1 stable facade's process-global lock remains
  separate native integration work;
- the first public-library slices authorized by the section 22 implementation
  plan, without pulling optional official ecosystems into `Pop.Standard`;
- optimization based on profiling and benchmarks.

Exit criterion: semantics and performance are stable enough for an initial
language release.

## Public-library delivery sequence

The detailed phase/package matrix, prerequisites, test and benchmark gates,
migration requirements, definitions of done, and first implementation pull
requests are maintained in
[Public library implementation and migration plan](./22.6-standard-library-implementation-plan.md).
This roadmap does not duplicate that catalog. A planned namespace is not an
implemented milestone artifact.

## Cross-cutting requirements

Every milestone includes:

- deterministic unit and snapshot tests;
- negative diagnostics tests;
- fuzzing for parsers and IR verifiers when they exist;
- compile-time interpreter determinism, cycle, visibility, and resource-limit
  tests;
- negative tests proving source-string injection and unrestricted reflection are
  unavailable;
- textual IR fixtures;
- performance baselines, not only peak benchmarks;
- documentation updates for architectural changes;
- CLI/manifest/lockfile and monorepo conformance tests;
- traceability from semantic features to accepted ADR/architecture sections;
- permanent negative tests for Lua regressions and architecture boundary leaks;
- architecture-gap review before any new public behavior is declared stable;
- naming baselines that reject `Iter`/`iter.map` and preserve
  `Iterable`/`Iterator`/`Sequence`;
- documentation conformance tests for every public standard API.
