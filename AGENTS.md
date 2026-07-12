# Pop Lang Agent Instructions

## Operating model: keep the contract active

Treat this file as an active operating contract, not as passive background
context. Critical rules must remain in the agent's active working set throughout
the task, especially while making design decisions, writing tests, editing code,
and declaring completion.

Do not assume that a rule remains operational merely because it appeared earlier
in the context. Re-read and reactivate the relevant invariants at each decision
checkpoint. After long tool output, a context switch, or a substantial subtask,
restore the working set before continuing.

Do not compress this file into a lossy summary. The persistent working set below
is a navigation and reactivation layer; every detailed instruction later in this
file remains binding.

## Persistent working set

Keep all of these invariants active throughout every task:

1. **Architecture authorizes behavior.** The accepted architecture and latest
   accepted ADRs are the source of truth. Do not invent contracts or let existing
   code redefine them.
2. **Architecture precedes tests; tests precede implementation.** Close design
   gaps first, encode the accepted behavior in deterministic tests second, and
   implement the smallest conforming change third.
3. **Pop Lang remains Pop Lang.** Preserve its native, strongly and statically
   typed, Luau-shaped identity. Prevent JavaScript/Rust/C#/D/C++ syntax drift and
   release-blocking Lua regressions.
4. **No operational dynamic escape hatch.** Never introduce `Any`, `Dynamic`,
   unchecked lookup/calls, string-based resolution, implicit globals, or dynamic
   fallback opcodes.
5. **Semantic concepts remain distinct.** Preserve the Item → Module → Bubble →
   Package → Workspace hierarchy and do not collapse records, classes, tables,
   namespaces, Modules, Bubbles, or Packages into one runtime mechanism.
6. **Backends share one semantic contract.** Keep HIR and MIR backend-neutral;
   MIR governs LLVM, the MIR interpreter, and future VM behavior.
7. **Compile time and reflection stay constrained.** Preserve deterministic,
   budgeted, capability-limited compile-time execution and the absence of
   unrestricted runtime reflection.
8. **Preserve work and verify honestly.** Keep user changes, make focused edits,
   run checks proportional to risk, and never claim a check passed unless it was
   actually run.

When two possible actions differ, prefer the one that preserves more of these
invariants simultaneously. When an action would violate one, stop and resolve the
architecture or test inconsistency instead of silently proceeding.

## Mandatory task loop

Use this loop for every change:

1. **Orient:** identify the requested outcome, affected ownership boundaries,
   public contracts, and likely architectural impact.
2. **Load authority:** read the required architecture documents, directly related
   documents, accepted ADRs, and closed decisions.
3. **Search broadly:** use `rg`/`rg --files` to find every affected term, example,
   decision, diagnostic, test, and cross-reference.
4. **State the authorized behavior internally:** distinguish accepted behavior,
   open questions, architecture gaps, and implementation details.
5. **Reactivate the persistent working set:** verify that the intended approach
   still preserves every applicable invariant above.
6. **Add tests before implementation:** make the pre-feature implementation fail
   for the intended missing behavior.
7. **Implement minimally:** make the smallest focused change that satisfies the
   accepted contract and tests.
8. **Re-scan and synchronize:** remove contradictory terminology and update all
   affected architecture, examples, decisions, diagnostics, and conformance
   material.
9. **Validate:** run the narrowest sufficient checks and the mandatory
   architecture-regression checks for the change.
10. **Report truthfully:** state what changed, what passed, what was not run, and
    any remaining architecture gap.

## Reactivation checkpoints

Pause and reload the relevant detailed sections of this file:

- after reading large files or long tool output;
- after switching between architecture, tests, and implementation;
- before changing any public language, library, runtime, artifact, diagnostic,
  tooling, or compatibility contract;
- before selecting syntax, naming, ownership, visibility, IR, runtime, GC,
  reflection, or library design;
- before modifying or accepting a test expectation;
- before declaring the task complete.

At each checkpoint, ask internally:

- What accepted architecture authorizes this exact decision?
- Which persistent invariants are active here?
- Am I accidentally treating implementation, convenience, or an open question as
  authority?
- What positive, negative, regression, consistency, and cross-backend evidence is
  required?
- What contradictory old model must be removed or synchronized?

## Stop conditions

Stop implementation and resolve the issue first when:

- no accepted architecture or ADR authorizes the proposed public behavior;
- an open design question would need to be answered silently;
- accepted architecture, tests, and implementation disagree;
- a cross-cutting change lacks the required ADR and synchronized documentation;
- the approach introduces dynamic typing, Lua table-centered semantics, syntax
  drift, backend-specific HIR/MIR, unrestricted reflection, or another forbidden
  regression;
- the behavior cannot yet be verified deterministically;
- completing the change would require deleting, weakening, ignoring, or rewriting
  a valid failing test merely to make implementation pass.

# Avoid Using Python

Whenever possible, do not use Python for scripting. Prefer Ruby when it is available.

For scripts intended to be added to the repository, ask the host machine owner to install Ruby if it is not already installed, and provide clear installation instructions for their operating system. We don't accept python scripts on repository.

# Follow Linux Git Commit Conventions

Write commit messages in the style commonly used by the Linux project. Begin with a short, specific subject line written in the imperative mood, such as `Fix invalid configuration handling` rather than `Fixed invalid configuration handling`. Do not end the subject line with a period.

Keep the subject concise and make it describe one logical change. Leave a blank line between the subject and the commit body.

Use the body to explain why the change was necessary, what problem it solves, and any important consequences. Do not merely repeat what the code already shows. Wrap body text at approximately 72 characters per line.

```text
Fix invalid configuration handling

Reject empty configuration values before initialization. Allowing them
to reach the parser caused unclear runtime errors and made invalid
deployments harder to diagnose.
```

Keep unrelated changes in separate commits. A bug fix, refactor,
formatting change, dependency update, and new feature should not be
combined unless they are inseparable parts of the same logical change.

Each commit should be independently understandable, reviewable, and
revertible. It should leave the repository in a working state whenever
practical.

Do not use vague commit subjects such as `Update code`, `Fix stuff`,
`Changes`, or `WIP`. Describe the actual change instead.

Bad:

Update files

Good:

Prevent duplicate user registration

## Scope

**Keep active:** accepted architecture is the repository contract, and the
canonical product/tool names are fixed.

This file applies to the entire repository.

The repository is currently architecture-first. The documents under
`architecture/` are the project contract and source of truth. Do not invent an
implementation, dependency, grammar rule, runtime behavior, artifact contract,
or compatibility promise that is not authorized by that architecture.

The product name is **Pop Lang** in English prose. Do not write `PopLang`,
`Pop language`, or translate the product name. Source files use `.pop`; the
unified command is `pop`.

## Required reading before changes

**Keep active:** do not edit before loading the authoritative architecture,
related ADRs, closed decisions, and affected references.

Before making a change, read:

1. `architecture/README.md`;
2. `architecture/19-architecture-conformance-and-regression-policy.md`;
3. the architecture documents directly related to the task;
4. relevant accepted ADRs under `architecture/decisions/`;
5. `architecture/08.1-closed-design-questions.md` when the topic was previously
   decided.

`architecture/08-open-design-questions.md` records questions, not permission to
choose an answer silently. A proposal, prototype, issue, comment, or convenient
implementation does not override accepted architecture.

Use `rg`/`rg --files` to find every affected term, example, decision, and
cross-reference before editing.

## Authority and change policy

**Keep active:** authority flows from accepted ADRs and architecture toward
tests and implementation, never in the opposite direction.

The precedence order is:

1. latest accepted ADR;
2. integrated architecture documents;
3. future language/library/runtime specifications;
4. conformance tests;
5. implementation.

If these disagree, the disagreement is a bug. Code does not get to redefine the
architecture merely because it exists.

Architecture is binding but evolvable. A cross-cutting change must:

- identify the existing decision being changed;
- add or amend an ADR;
- update every affected architecture document;
- update canonical examples and nomenclature;
- update `architecture/08.1-closed-design-questions.md` when it closes or revises
  a design question;
- define positive, negative, regression, and cross-backend conformance tests;
- remove contradictory old terminology instead of leaving two models active.

If a behavior is not designed well enough to verify, report an architecture gap
and design it before treating it as stable. Do not hide a new public contract in
an implementation detail.

Anything outside accepted architecture is a bug until an ADR deliberately
changes the baseline. Turning Pop Lang back into Lua's dynamic/table-centered
architecture is a release-blocking **Lua regression**.

## Architecture-to-test-to-implementation workflow

**Keep active:** every feature follows Architecture → Tests → Implementation,
with no implementation-first exception.

Every feature and behavior follows this mandatory order:

1. **Architecture:** identify the authorizing architecture section and accepted
   ADR. Close any architecture gap before implementation.
2. **Tests:** add deterministic tests that encode the accepted behavior,
   conventions, consistency rules, boundaries, and forbidden regressions. Run
   them against the pre-feature implementation and confirm that they fail for
   the intended missing behavior.
3. **Implementation:** write the smallest implementation that satisfies the
   accepted architecture and makes the new tests pass.

No feature implementation may precede its tests. Every feature needs positive,
negative, convention, consistency, and regression coverage proportional to its
architectural impact. Cross-backend features also need differential or shared
conformance coverage.

Tests are executable architecture contracts. Do not skip, weaken, delete,
rename, ignore, or rewrite a failing test merely to make an implementation pass.
An expected result may change only after the authorizing architecture and, when
required, its ADR change first. Fix the implementation when it disagrees with an
accepted test. If a test contradicts the accepted architecture, stop and repair
the architecture/test inconsistency before continuing to code.

## Language identity

**Keep active:** Pop Lang is native, strongly and statically typed, and must
remain a natural Luau extension.

Pop Lang is a native, strongly and statically typed language directly inspired
by Luau.

Preserve Luau's readable character:

- lightweight syntax;
- `local`, `function`, colon methods, and `end` blocks;
- Luau-style type annotations and generic-call direction;
- table/array literal beauty for actual typed collections;
- first-class functions, closures, coroutines, and local inference;
- low punctuation and little ceremony.

Do not drift toward JavaScript, Rust, C#, D, or C++ surface syntax. Pop may adopt
their architectural ideas, but syntax must remain a natural Luau extension.
Braces are data/initializer literals, not declaration blocks. Semicolons and
JavaScript import/export syntax are not part of canonical Pop source.

## Strong static typing

**Keep active:** every runtime operation has a compiler-proven type; no
operational dynamic fallback is permitted.

Every runtime value and operation has a compiler-proven type.

Never introduce:

- `Any`, `Dynamic`, or another operational unknown/dynamic value;
- inference failure that becomes runtime typing;
- unchecked member lookup or calls;
- runtime member/type/function resolution from strings;
- untyped heterogeneous collections;
- untyped variadic/multiple results;
- implicit globals;
- dynamic fallback opcodes in HIR or MIR.

Use explicit unions, nominal interfaces, optional/result types, typed tables,
checked casts, parsers/decoders, or typed unsafe FFI boundaries instead.

## Native abstractions and non-OOP default

**Keep active:** semantic concepts stay distinct, and functions/data are
preferred over unnecessary OOP structures.

Classes, records, unions, tuples, arrays, tables, Modules, namespaces, Bubbles,
and Packages are distinct semantic concepts. Do not secretly implement their
observable behavior through universal tables, metatables, or runtime hashes.

Prefer abstractions in this order:

1. local values and plain functions;
2. records and tagged unions;
3. arrays/tables plus generic algorithms;
4. Modules and namespaces;
5. composition and function/capability values;
6. small nominal interfaces for real polymorphic boundaries;
7. classes for stable identity, encapsulated mutable lifecycle, or required
   runtime dispatch;
8. inheritance only for deliberate substitutability and shared implementation.

Do not create static utility classes, service/factory/helper/manager classes,
singleton namespace objects, module return tables, marker interfaces, or fluent
object graphs when namespace functions and data express the design.

Functions may live directly in namespaces.

## Namespaces and visibility

**Keep active:** one file-scoped namespace per Module, explicit namespace-scope
visibility, and no export/re-export model.

Every `.pop` Module declares one file-scoped namespace. A namespace is a static
name scope, not a runtime value, table, Bubble, Package, or filesystem folder.

Every namespace-scope declaration explicitly uses exactly one of:

- `public`: visible to dependent Bubbles and emitted in reference metadata;
- `internal`: visible across Modules in the same Bubble only;
- `private`: visible only in the declaring Module/file.

`local` remains block/function-local. Namespace declarations themselves have no
visibility modifier.

Pop Lang has no `export` prefix, export list, or re-export mechanism. `using`
changes compile-time name lookup only. It never creates dependencies, loads
code, forwards visibility, or becomes a runtime operation.

## Naming and aesthetics

**Keep active:** canonical casing, complete readable names, accepted technical
forms, and Luau-shaped aesthetics are contractual.

Canonical Pop source uses:

- `PascalCase` for namespaces, Packages, Bubbles, types, interfaces, enum/union
  cases, type parameters, and user-defined/compiler attributes;
- `camelCase` for functions, methods, fields, locals, parameters, Modules, and
  source filenames;
- `UPPER_SNAKE_CASE` only for constants;
- `_` only for an intentionally ignored binding.

Lowercase `snake_case` is not allowed in Pop source.

Use complete readable words. Do not introduce arbitrary truncations such as
`Iter`, `Config`, `Sync`, `Mgr`, or `Util`. The sequence protocols are
`Iterable<T>` and `Iterator<T>`; algorithms are `Sequence.map`,
`Sequence.filter`, and `Sequence.fold`. `Iter`/`iter.map` is forbidden.

Established technical forms such as `Json`, `Http`, `Io`, `Utf8`, `Ffi`, `Gc`,
`Guid`, and `Async` are accepted and cased as words. New exceptions require
architecture review.

Attributes are PascalCase. Write `@Serializable`, `@CompileTime`,
`@SuppressWarning`, and `@RetainMetadata`, never lowercase variants.

The reserved tooling paths `src/`, `src/lib.pop`, and `src/bin/` are deliberate
filesystem conventions requested by the Package model. They do not authorize
`Src`, `Lib`, `Bin`, or other truncated Pop identifiers.

## Units of code and tooling

**Keep active:** Item → Module → Bubble → Package → Workspace defines ownership,
visibility, compilation, and tooling terminology.

The fixed ownership hierarchy is:

```text
Item → Module → Bubble → Package → Workspace
```

- An Item is a declaration/member/case.
- A Module is one `.pop` file and the `private` boundary.
- A Bubble is the crate-like independent compilation/reference/linking and
  `internal` boundary.
- A Package is a publishable/versioned directory whose `bubble.toml` contains
  `[package]`.
- A Workspace groups Packages under one resolver/lock/cache/policy root without
  merging visibility or compilation boundaries.

Two Bubbles in the same Package or Workspace interact through declared
dependencies and public APIs. Package/Workspace membership never widens
`internal`.

The conventional layout is:

```text
bubble.toml
src/lib.pop
src/main.pop
src/bin/
tests/
examples/
benchmarks/
```

Workspaces share one deterministic `bubble.lock` and `target/` output/cache root.
Support normal, development, platform, registry, exact-Git, and local-path
dependencies through the resolved Package/Bubble graph. Paths are resolution
inputs, never semantic identity.

The unified user-facing tool is `pop`. Prefer complete commands and options:
`pop check`, `pop build`, `pop run`, `pop test`, `pop benchmark`, `pop
documentation`, `pop format`, `pop lint`, `pop fix`, `pop add`, `pop remove`,
`pop update`, `pop tree`, `pop metadata`, `pop package`, and `pop publish`.
Do not introduce abbreviated primary commands such as `fmt`, `bench`, or `doc`.

Machine tooling consumes versioned structured diagnostics, metadata, build
events, symbol IDs, and workspace edits. It must not scrape human CLI output.

## Compiler architecture

**Keep active:** preserve the required semantic pipeline, typed stable IDs,
verified IR stages, and backend-neutral HIR/MIR.

The required semantic pipeline is:

```text
Source → tokens → lossless syntax tree → declaration index → resolved AST
→ typed/compile-time analysis → HIR → canonical MIR → backend
```

Rules:

- HIR and MIR are backend-neutral and contain no LLVM objects/opcodes/layouts.
- HIR preserves typed language concepts and resolved stable IDs.
- MIR makes control flow, evaluation order, calls, effects, failures, GC safe
  points, and runtime operations explicit.
- MIR is the contract for LLVM, the MIR interpreter, and a future VM backend.
- A backend cannot call back into parsing, resolution, typing, or compile-time
  evaluation.
- Backend semantic disagreement is a bug unless caused by a documented target
  capability.
- Every IR construction/transformation stage is verified.

Use `WorkspaceId`, `PackageId`, `BubbleId`, `ModuleId`, and typed entity IDs.
Compiler/query terminology must respect the ownership hierarchy; use
`HirBubble`, `MirBubble`, `BubbleIdentity`, and `BubbleContext`, not obsolete
library-as-compilation-unit names.

The compiler implementation uses Rust edition 2024 and the accepted Cargo
workspace boundaries from ADR 0018. This host-language choice does not authorize
Rust surface syntax or replace Pop Lang's own Package/Bubble model.

## UDAs, compile time, and reflection

**Keep active:** compile time is deterministic and capability-limited; runtime
reflection is absent by default.

User-defined attributes are nominal, typed, immutable compile-time values.
Compile-time execution is deterministic, budgeted, capability-limited, and
dependency-tracked.

Never add:

- string mixins or text-to-source generation;
- `eval` or source parsing/injection at compile time;
- ambient filesystem, network, process, clock, random, or environment access;
- attribute-driven grammar/tokenization changes;
- unrestricted symbol/type enumeration;
- runtime get/set/call-by-name reflection;
- compiler/backend handles escaping into runtime values.

Runtime reflection is absent by default. Explicit retained metadata must be a
narrow serializable projection consumed through generated typed adapters.

## Runtime, GC, and ABI

**Keep active:** PLRI and the accepted precise concurrent generational GC model
are cross-backend semantic contracts.

Generated code reaches runtime services through the versioned backend-neutral
Pop Lang Runtime Interface (PLRI).

Pop GC is a precise concurrent generational collector with a moving nursery,
mostly non-moving mature heap, precise roots/stack maps, safe points, SATB and
generational barriers, and bounded pause work. Do not casually add finalizers,
weak references, resurrection, conservative scanning, untracked raw managed
pointers, or unloading without the accepted GC proof obligations.

Native and future VM backends must preserve the same object, initialization,
visibility, metadata, error, and GC semantics.

## Base libraries

**Keep active:** the foundational library model consists exactly of
`Pop.Internal` and `Pop.Standard` with the prescribed dependency direction.

The toolchain supplies exactly two reserved foundational library Bubbles:

- `Pop.Internal`: trusted compiler/runtime primitives, intrinsics, GC/ABI
  bridges, and platform adapters; never directly referenced by user code;
- `Pop.Standard`: public compact BCL-inspired APIs and the fixed curated prelude.

`Pop.Standard` depends on `Pop.Internal`; the inverse is forbidden. Do not copy
.NET BCL APIs unchanged. Adapt them to Pop's static, typed-error, UTF-8,
restricted-reflection, function/data-first, compact-namespace design.

Library Bubbles emit self-describing `.poplib` artifacts with
`bubble.manifest`, public `reference.metadata`, separate `documentation.xml`,
target implementations, hashes, ABI/capability information, and exact Bubble
dependencies. Only public declarations enter consumer metadata.

## Diagnostics and fixes

**Keep active:** diagnostics are structured semantic APIs; fixes must preserve
architecture, safety, and atomic verification.

Diagnostics are structured APIs with stable `POP####` codes, typed arguments,
spans/labels/notes/origins, intrinsic severity/category, warning waves,
suppression policy, and semantic quick fixes.

Do not:

- emit final strings directly from compiler passes;
- parse diagnostic messages to recover semantic facts;
- hide compiler/architecture incidents as user errors;
- suppress errors or Lua regressions;
- offer `Any`, dynamic lookup, unsafe casts, or reflection as fixes;
- auto-apply review/unsafe fixes;
- download/add dependencies as an ordinary unapproved source fix.

Safe fix-all must be atomic, version-checked, composable, formatted, and verify
its postcondition. CLI, LSP, JSON, SARIF, and tests render the same diagnostic
object.

## XML documentation

**Keep active:** documentation is checked, safe, separate from runtime
reflection, and part of the public contract.

Pop XML documentation uses Lua-shaped `---` comments and checked XML concepts
inspired by C#.

- Documentation precedes attributes and declarations.
- XML is parsed with DTD/entities/external resolution disabled.
- Parameters, type parameters, returns, typed errors, effects, complexity,
  allocation, thread safety, and `cref` links are semantically validated.
- `<code>` is documentation/test input, never a macro or string mixin.
- Public `Pop.Standard` APIs require complete checked documentation and compiled
  nontrivial examples.
- Documentation is emitted separately and does not enable runtime reflection.

## Editing rules

**Keep active:** preserve unrelated work, make focused edits, synchronize
terminology, and avoid generated or local artifacts.

- Preserve user changes and unrelated work.
- Prefer focused `apply_patch` edits; do not perform destructive resets or broad
  mechanical rewrites without justification.
- Keep architecture documents in English.
- Use CommonMark/GitHub Markdown with blank lines around headings and lists.
- Keep examples beautiful, minimal, strongly typed, Luau-shaped, and canonical.
- Treat an incorrect architecture example as a documentation bug.
- Update links and terminology across the repository when renaming a concept.
- Use primary/official references for external technical claims.
- Do not add generated artifacts, build outputs, dependency caches, credentials,
  or local editor files.

## Validation before completion

**Keep active:** completion requires proportional verification and explicit
honesty about checks not run.

For architecture changes, at minimum verify:

- all relative Markdown links resolve;
- every Luau namespace-scope example uses explicit visibility;
- no example uses `export` syntax;
- no lowercase attribute names appear;
- no forbidden dynamic types/operations were introduced;
- no `Iter`/`iter.map` or arbitrary truncation became canonical;
- Item/Module/Bubble/Package/Workspace terminology remains consistent;
- HIR/MIR stay backend-neutral;
- affected ADRs, closed decisions, roadmap, diagnostics, examples, and
  conformance matrices agree.

When implementation exists, also run the narrowest relevant formatter, unit,
conformance, integration, cross-backend, and architecture-regression suites.
Do not claim tests passed unless they were actually run.

## Definition of done

**Keep active:** done means the requested outcome, architecture, terminology,
examples, tests, and stated verification all agree.

A task is complete only when:

- the requested outcome is implemented or documented;
- the result conforms to accepted architecture;
- no contradictory old model remains active;
- relevant examples and terminology are synchronized;
- verification proportional to the risk has passed;
- remaining architecture gaps or unrun checks are stated clearly.