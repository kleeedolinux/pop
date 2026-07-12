# Architecture Conformance and Regression Policy

## Rule

The Pop Lang architecture is a **binding, evolvable baseline**.

- Binding: compiler, runtime, standard library, backends, tools, specifications,
  examples, and tests must conform to accepted architectural decisions.
- Evolvable: the project can revise a decision through an explicit proposal/ADR
  and coordinated documentation/tests before the new behavior becomes normal.

An implementation does not get to redefine the architecture by existing. If it
contradicts accepted architecture, the implementation is buggy until the code is
corrected or the architecture is deliberately superseded.

An uncovered semantic/public behavior is an architecture gap. It must be
designed before it becomes a stable feature. Implementation convenience,
performance, compatibility, or schedule pressure is never an implicit waiver.

## Scope

This policy governs decisions visible across or between components:

- language syntax and semantics;
- static type-system acceptance;
- HIR/MIR contracts and operations;
- compile-time execution and UDA capabilities;
- reflection/metadata limits;
- Modules, namespaces, Bubbles, Packages, Workspaces, manifests, locks, and loading;
- runtime ABI, GC, object identity, FFI, and coroutine behavior;
- backend equivalence;
- `Pop.Internal` intrinsics and `Pop.Standard` public APIs/prelude;
- diagnostics, warning policy, and quick-fix safety;
- naming, paradigm/API style, and compatibility direction;
- unified CLI, Bubble discovery, dependency resolution, and monorepo behavior;
- serialized/public artifact formats and versioning promises.

Private implementation details remain free when they satisfy every relevant
invariant. Examples include arena data structures, hash-table algorithms,
compiler work-queue layout, and a backend-private temporary IR. A private detail
becomes architectural when another component, public artifact, observable
behavior, or compatibility promise relies on it.

## Authority and precedence

The project keeps these layers consistent:

1. accepted ADRs establish/revise decisions and rationale;
2. current architecture documents describe the integrated design;
3. the language/library/runtime specifications detail user-visible behavior;
4. conformance tests encode required behavior and forbidden regressions;
5. implementation realizes those contracts.

If these disagree, the disagreement itself is a bug. The latest accepted ADR
identifies the intended decision, but accepting it must include updates to every
contradicting architecture/spec/test. Code cannot choose its favorite layer.

An open question, proposal, issue comment, prototype, or provisional branch does
not override accepted architecture.

## Bug classes

### Architecture drift

`ArchitectureDrift` is any code, API, test, or document that contradicts an
accepted invariant or bypasses a required boundary.

Examples:

- HIR/MIR contains LLVM-specific types;
- a backend reconstructs source semantics instead of consuming canonical MIR;
- a public API returns an untyped value;
- compile-time code reads ambient filesystem/network state;
- runtime metadata exposes fields without an accepted retention adapter;
- `Pop.Standard` depends on private compiler packages;
- an unsafe quick fix is silently included in fix-all;
- a class/module is secretly implemented through observable table/metatable
  behavior.

### Architecture gap

`ArchitectureGap` is a proposed or implemented semantic/public choice not
covered well enough to verify conformance.

Examples:

- adding weak references without reachability/ephemeron semantics;
- adding declaration-generating macros without phase/hygiene rules;
- exposing a new artifact format without versioning guarantees;
- shipping a new standard prelude name without compatibility review;
- adding implementation inheritance behavior beyond the accepted class model.

The correct response is to stop stabilization, write the design/ADR and tests,
then implement. The absence of a prohibition is not automatic permission for a
new cross-cutting contract.

### Backend drift

`BackendDrift` occurs when LLVM, MIR interpreter, or VM produces different
observable Pop Lang semantics except for documented target capabilities.
Backend-specific performance/layout choices are allowed; semantic disagreement
is a bug.

### Library drift

`LibraryDrift` occurs when `Pop.Internal`/`Pop.Standard` violates layering,
naming, prelude, strong typing, non-OOP API style, error, reflection, allocation,
or cross-backend contracts.

Treating a foreign platform library as Pop Lang's API or object-model template
is library drift. Mature libraries may only inform coverage checklists; ADR 0030
and the public standard-library architecture define the public contract.

### Documentation drift

Architecture examples and explanatory text can create implementation pressure.
An example that uses lowercase types, non-PascalCase attributes, repeated
standard imports, dynamic operations, or unnecessary OOP shapes is a bug in the
documentation and should be fixed like code.

## Lua regression: a specific release-blocking bug

Pop Lang is inspired by Luau's syntax, ergonomics, and tooling goals. It is not
permitted to slide back into Lua's semantic architecture.

A **Lua regression** is a change that makes Pop Lang use Lua compatibility or
table/dynamic conventions in place of an accepted native/static Pop construct.

The following are Lua regressions:

- adding `Any`, `Dynamic`, or an operational unknown type;
- allowing inference failure to become runtime typing;
- using tables as universal objects, records, tuples, classes, namespaces, or
  module public-symbol containers;
- implementing ordinary class fields/methods through metatable/hash lookup;
- reintroducing metatables/metamethods as the general extension mechanism;
- making modules return runtime table values through `require`;
- permitting implicit globals or mutable function/module environments;
- resolving program members/types/functions from runtime strings;
- allowing undeclared fields to appear on normal class instances;
- adding untyped multiple/variadic results;
- bypassing nominal interfaces with runtime duck typing;
- exposing broad runtime reflection because Lua-style dynamic access needs it;
- weakening errors/typing/layout to accept more Lua source automatically;
- making a table idiom the canonical standard-library abstraction when a record,
  union, function, namespace, protocol, or class has the real semantics.

A Lua regression is not accepted as a migration shortcut, performance hack, or
temporary compatibility mode in normal builds. It blocks release until removed
or until an explicit project-level architectural re-foundation is debated and
accepted—which would be a change to Pop Lang's identity, not an ordinary ADR.

## What remains intentionally Luau-like

These are not Lua regressions when their Pop semantics remain static/native:

- lightweight lexical syntax and `end` blocks;
- `local`, `function`, colon method ergonomics, multiple assignment;
- table/array literal beauty for actual typed collections;
- closures, first-class functions, coroutines, and generalized iteration;
- if-expressions, compound assignment, interpolation, and type annotations;
- small readable programs with low ceremony;
- fast analysis, actionable diagnostics, and migration familiarity.

The rule is simple: preserve the beautiful surface where it fits; do not restore
the hidden dynamic/table machinery underneath it.

## Required change process

Any change that would exceed or contradict architecture follows this order:

1. **Describe the problem** and why current architecture is insufficient.
2. **Classify impact** across syntax/types/HIR/MIR/runtime/backends/libraries/
   tooling/security/compatibility.
3. **Write a proposed ADR** with alternatives and consequences.
4. **Update integrated architecture documents** in the same change.
5. **Define conformance and negative regression tests** before stabilization.
6. **Accept/reject the ADR** through project review.
7. **Implement behind the accepted contract**.
8. **Remove superseded paths/tests/docs** so two architectures do not coexist.

For ordinary feature delivery inside accepted architecture, the mandatory local
sequence is architecture traceability, a failing deterministic test for the
missing behavior, and then implementation. Convention, consistency, negative,
and regression coverage are part of the feature rather than optional follow-up
work. A test expectation is not weakened to accommodate contradictory code; the
implementation is corrected unless an accepted architecture change authorizes a
new expectation first.

If implementation work reveals that an accepted design is impossible or harmful,
work stops at the contradictory boundary and opens an architecture change. The
code does not silently choose a new policy.

### Experimental work

Research prototypes may explore alternatives only when isolated from default
language/library behavior:

- clearly marked experimental branch/feature;
- not included in stable `Pop.Standard`/artifacts;
- no compatibility promise;
- no replacement of conformance tests;
- linked proposal explaining which architecture it tests;
- removed or converted through the normal ADR process before release.

An experiment that leaks into normal builds is architecture drift.

## Decision traceability

Cross-cutting implementation/tests should reference the relevant ADR or
architecture section in their design metadata/test name/documentation—not every
source line, but every semantic feature boundary.

Minimum traceability matrix:

| Decision | Owning components | Required proof |
| --- | --- | --- |
| Strong static typing | resolver, type checker, HIR/MIR | no dynamic fallback/opcode tests |
| Native classes | type checker, HIR/MIR, runtime | resolved field/method IDs; no table lookup |
| Bubbles/Packages/Workspaces | project resolver, manifest, driver, loader | identity/init/lock/target-selection tests |
| Unified `pop` tooling | CLI, language server, formatter, docs, package manager | command/JSON/selection/reproducibility tests |
| Default internal visibility | parser, resolver, HIR, metadata | default/access/public-surface tests |
| Complete public names | formatter, analyzers, libraries | naming baseline and truncation diagnostics |
| Backend-neutral MIR | MIR, interpreter, LLVM, VM | verifier plus cross-backend conformance |
| Restricted UDAs/compile time | compile-time engine | capability/string-injection negative tests |
| Restricted reflection | compiler/runtime/base libraries | no runtime enumerate/get-by-name tests |
| Non-OOP default | examples, public library, analyzers | API baseline and `ApiDesign` tests |
| Native public library tiers | package resolver, public library, documentation | tier graph, namespace, capability, and forbidden-pattern tests |
| Rust foundation adapters | library bridge/macro, base libraries, bootstrap verifier | closed `#[poplib]` descriptors, exact ABI checks, explicit inventories, no runtime registration |
| Concise APIs and explicit costs | public libraries, docs, analyzers, benchmarks | call-site, allocation/copy/dispatch, effect, and measured-budget tests |
| Independent official extensions | package resolver, extension builds, tooling | manifest/version/dependency/namespace/standard-exclusion tests |
| Compact prelude | resolver, `Pop.Standard` | exact prelude snapshot/collision tests |
| Pop GC | compiler/runtime/backends | root/barrier/stress/latency correctness tests |
| Diagnostic fixes | compiler/tooling | applicability, atomicity, postcondition tests |
| XML documentation | parser, resolver, docs, libraries | tag/signature/cref/artifact tests |
| No Lua regression | all semantic layers | permanent forbidden-feature regression suite |

## Review checklist

Every semantic/public pull request answers:

- Which architecture section/ADR authorizes this behavior?
- Does it introduce a new public contract not currently covered?
- Does it add dynamic typing, string lookup, reflection, or table universality?
- Does it couple HIR/MIR to LLVM or one runtime implementation?
- Does it make LLVM/VM/interpreter semantics diverge?
- Does it add OOP ceremony where data/functions/composition suffice?
- Does it add long/redundant standard names or repeated imports?
- Does it change prelude/API/ABI/artifact compatibility?
- Which positive/negative conformance tests prove the boundary?
- Which architecture/spec documents need synchronized updates?

“No architecture impact” must be defensible, not a way to skip review.

## CI and release gates

Architecture CI should eventually verify:

- architecture links, numbering, ADR status, and document consistency;
- traceability entries for new semantic features;
- forbidden HIR/MIR dynamic/LLVM-leak operations;
- exact `Pop.Standard` prelude/API baselines;
- `Pop.Internal`/`Pop.Standard` dependency direction;
- modular base-library module/test ownership and thin crate-root inventories;
- typed public-function reference metadata and Bubble-scoped symbol identities;
- naming/PascalCase/no-lower-snake rules;
- default-internal visibility, private binary-entry shorthand, and absence of
  export/re-export syntax;
- complete-word API rules, including `Iterable`/`Iterator`/`Sequence` baselines;
- non-OOP standard API analyzer checks;
- complete public root inventory with unique ownership/tier/status;
- concise default/advanced/efficient call-site examples for each API family;
- allocation, copying, view, dispatch, blocking/suspension, and native-boundary
  contract checks;
- compile-time/reflection capability negative tests;
- native class field/method lowering without table/metatable lookup;
- Module/Bubble behavior without `require` value tables;
- `bubble.toml`/`bubble.lock` and monorepo selection/reproducibility tests;
- cross-backend semantic conformance;
- permanent Lua-regression test corpus;
- no unresolved architecture gaps in release features;
- checked public documentation/API signature consistency for base libraries.

A failing gate is not waived by updating expected output to match buggy behavior.
The expectation changes only with an accepted architecture change.

## Bug handling

Architecture bug reports include:

- violated decision/document;
- observed code/API/behavior;
- affected compiler/runtime/library/backend versions;
- minimal reproduction or public API diff;
- whether it is architecture drift, gap, backend/library/documentation drift, or
  Lua regression;
- safety/compatibility/artifact impact;
- proposed correction or ADR need.

Lua regressions, strong-type violations, ABI corruption, backend semantic drift,
and runtime safety violations are release blockers. Documentation/API-style
drift is fixed with normal bug priority but cannot be declared the intended
architecture merely because it shipped accidentally.

## Final invariant

Architecture is the project contract, not a suggestion and not a prison.

Change it deliberately when Pop Lang needs to grow. Until it is deliberately
changed, anything outside it is a bug—and turning Pop Lang back into Lua is one
of the clearest bugs the project can introduce.
