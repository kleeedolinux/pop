# Pop Lang Agent Contract

Applies repository-wide. Active contract, not background. Keep every rule active
through design, tests, code, validation, and completion.

## Communication

Use `caveman` skill when available and clarity permits; default `full` for
commentary/final responses. Use normal precise language for source, docs,
commits, PRs, exact errors, safety warnings, destructive confirmations, confused
users, or sequences where fragments risk ambiguity.

## Always-active invariants

1. Accepted architecture/latest ADR authorizes behavior; code never does.
2. Order: Architecture → failing deterministic Tests → minimal Implementation.
3. Preserve native, strongly/static typed, Luau-shaped Pop Lang; prevent
   JavaScript/Rust/C#/D/C++ syntax drift and release-blocking Lua regression.
4. No operational dynamic escape: `Any`, `Dynamic`, unchecked lookup/calls,
   runtime string resolution, implicit globals, dynamic fallback opcodes.
5. Keep Item → Module → Bubble → Package → Workspace and records/classes/
   tables/namespaces/Modules/Bubbles/Packages semantically distinct.
6. HIR/MIR remain backend-neutral; MIR governs LLVM, MIR interpreter, future VM.
7. Compile time remains deterministic/budgeted/capability-limited; no broad
   runtime reflection.
8. Preserve user work; focus edits; verify and report honestly.

If options conflict, choose one preserving most invariants. Stop when any would
be violated; resolve architecture/test inconsistency first.

## Mandatory loop

1. Orient: outcome, owners, public contracts, architecture impact.
2. Load authority: required architecture, related docs, accepted ADRs, closed
   decisions.
3. Search broadly with `rg`/`rg --files`: terms, examples, decisions,
   diagnostics, tests, cross-references.
4. Separate accepted behavior, open questions, architecture gaps, private
   implementation.
5. Recheck all active invariants.
6. Add tests first; confirm pre-feature failure is the intended missing behavior.
7. Implement smallest conforming change.
8. Re-scan; remove contradictory old model; synchronize docs/examples/
   decisions/diagnostics/conformance.
9. Run narrow sufficient checks plus required architecture regressions.
10. Report changes, passed checks, unrun checks, remaining gaps.

Reactivate relevant rules after long output/context switches/subtasks; between
architecture/tests/implementation; before public syntax/library/runtime/
artifact/diagnostic/tool/compatibility changes; before syntax/naming/ownership/
visibility/IR/runtime/GC/reflection/library decisions; before changing test
expectations; before completion.

At each checkpoint ask: Which accepted architecture authorizes this? Which
invariants apply? Am I treating convenience/code/open questions as authority?
What positive/negative/regression/consistency/cross-backend proof is needed?
Which old model must disappear?

Stop implementation if authority is missing; an open choice would be answered
silently; architecture/tests/code disagree; a cross-cutting change lacks ADR and
synchronized docs; dynamic typing/table semantics/syntax drift/backend-specific
HIR/MIR/unrestricted reflection would enter; behavior is not deterministically
verifiable; or passing requires weakening/deleting/ignoring/rewriting a valid
failing test.

## Authority

Read before changes:

1. [`architecture/README.md`](architecture/README.md);
2. [conformance policy](architecture/19-architecture-conformance-and-regression-policy.md);
3. directly related architecture;
4. relevant accepted ADRs under `architecture/decisions/`;
5. [closed decisions](architecture/08.1-closed-design-questions.md) for decided
   topics.

`architecture/08-open-design-questions.md` asks questions; it grants no choice.
Proposals, prototypes, issues, comments, convenience, and implementation do not
override accepted architecture.

Precedence: latest accepted ADR → integrated architecture → future
language/library/runtime specifications → conformance tests → implementation.
Disagreement is a bug.

Cross-cutting architecture change must identify superseded decision; add/amend
ADR; update all architecture, canonical examples/nomenclature, and closed
decisions; define positive/negative/regression/cross-backend conformance; remove
the contradictory model. Undesigned public behavior is an architecture gap:
design before stabilization. Anything outside accepted architecture remains a
bug until ADR changes baseline. Lua regression blocks release.

## Tests before code

Every feature: trace authorizing architecture/ADR; close design gaps; add
deterministic behavior/convention/consistency/boundary/regression tests and
confirm intended pre-feature failure; then implement minimally. Cross-backend
features need shared/differential coverage.

Tests are executable architecture. Never skip, weaken, delete, rename, ignore,
or rewrite valid failures to pass code. Change expectations only after
authorizing architecture/ADR. Fix implementation when it conflicts. If test
conflicts with architecture, repair that inconsistency before coding.

## Language contract

Pop Lang is native, strongly/static typed, directly Luau-inspired. Preserve
light syntax; `local`, `function`, colon methods, `end`; Luau annotations and
generic-call direction; typed table/array literal beauty; functions, closures,
coroutines, local inference; low punctuation/ceremony. Braces mean data/
initializer literals, never declaration blocks. No semicolon or JavaScript
import/export style.

Every runtime value/operation has compiler-proven type. Never add operational
unknown/dynamic values; inference-to-runtime typing; unchecked lookup/calls;
runtime member/type/function string resolution; heterogeneous untyped
collections; untyped variadic/multiple results; implicit globals; dynamic HIR/
MIR opcodes. Use explicit unions, nominal interfaces, optional/results, typed
tables, checked casts, parsers/decoders, typed unsafe FFI boundaries.

Keep class, record, union, tuple, array, table, Module, namespace, Bubble,
Package distinct; never expose universal tables/metatables/runtime hashes.
Prefer: locals/plain functions → records/tagged unions → arrays/tables plus
generic algorithms → Modules/namespaces → composition/function/capability
values → small nominal interfaces for real polymorphism → classes for stable
identity/encapsulated mutable lifecycle/required dispatch → inheritance only
for deliberate substitutability/shared implementation.

No static utility classes; service/factory/helper/manager classes; singleton
namespace objects; module return tables; marker interfaces; fluent object graphs
where namespaces/functions/data suffice. Functions may live in namespaces.

C back end is experimental; do not spend major effort on full parity or treat it
as mainstream priority.

## Namespace, visibility, naming

Each `.pop` Module declares one file-scoped namespace: static scope, never
runtime value/table/Bubble/Package/folder. Namespace declarations have no
visibility modifier. Namespace-scope Items are exactly `public` (dependent
Bubbles/reference metadata), `internal` (same Bubble; default), or `private`
(same Module/file). Exact binary-root `main` shorthand alone defaults private.
`local` remains block/function-local.

No `export` prefix/list/re-export. `using` only changes compile-time lookup; it
never creates dependency, loads code, forwards visibility, or runs.

Canonical casing:

- `PascalCase`: namespaces, Packages, Bubbles, types, interfaces, enum/union
  cases, type parameters, user/compiler attributes.
- `camelCase`: functions, methods, fields, locals, parameters, Modules, source
  files.
- `UPPER_SNAKE_CASE`: constants only; `_`: intentionally ignored binding only.
- Never lowercase `snake_case` in Pop source.

Use complete readable words; no arbitrary `Iter`, `Config`, `Sync`, `Mgr`,
`Util`. Required: `Iterable<T>`, `Iterator<T>`, `Sequence.map/filter/fold`;
never `Iter`/`iter.map`. Accepted word-cased technical forms: `Json`, `Http`,
`Io`, `Utf8`, `Ffi`, `Gc`, `Guid`, `Async`; new exceptions need architecture
review. Attributes are PascalCase: `@Serializable`, `@CompileTime`,
`@SuppressWarning`, `@RetainMetadata`.

Reserved paths `src/`, `src/lib.pop`, `src/bin/` do not authorize truncated Pop
identifiers `Src`, `Lib`, `Bin`.

Product name: **Pop Lang** (never `PopLang`, `Pop language`, or translation).
Source: `.pop`; unified command: `pop`.

## Code units and tools

Fixed ownership: `Item → Module → Bubble → Package → Workspace`.

- Item: declaration/member/case.
- Module: one `.pop`, private boundary.
- Bubble: independent compile/reference/link and internal boundary.
- Package: publishable/versioned directory; `[package]` in `bubble.toml`.
- Workspace: Package resolver/lock/cache/policy root; never merges visibility or
  compilation.

Bubbles in same Package/Workspace still use declared dependencies/public APIs;
membership never widens `internal`. Paths resolve; they never define semantic
identity. Conventional layout: `bubble.toml`, `src/lib.pop`, `src/main.pop`,
`src/bin/`, `tests/`, `examples/`, `benchmarks/`. Workspace shares deterministic
`bubble.lock` and `target/`. Support normal/development/platform/registry/
exact-Git/local-path dependencies through resolved Package/Bubble graph.

Use complete commands: `pop check/build/run/test/benchmark/documentation/format/
lint/fix/add/remove/update/tree/metadata/package/publish`; no primary `fmt`,
`bench`, `doc`. Machine tools consume versioned structured diagnostics,
metadata, build events, symbol IDs, workspace edits; never scrape human output.

## Compiler and runtime

Pipeline: Source → tokens → lossless syntax tree → declaration index → resolved
AST → typed/compile-time analysis → HIR → canonical MIR → backend.

HIR/MIR contain no LLVM objects/opcodes/layout. HIR preserves typed concepts and
resolved stable IDs. MIR makes control flow, evaluation order, calls, effects,
failures, GC safe points, runtime operations explicit. Backends never call
parsing/resolution/typing/compile time. Backend semantic disagreement is bug
unless documented target capability. Verify every IR construction/transform.

Use `WorkspaceId`, `PackageId`, `BubbleId`, `ModuleId`, typed entity IDs;
`HirBubble`, `MirBubble`, `BubbleIdentity`, `BubbleContext`, never obsolete
library-as-compilation-unit terms. Compiler uses Rust 2024 and ADR 0018 Cargo
workspace; host Rust never authorizes Rust-like Pop syntax or replaces Pop
Package/Bubble model.

UDAs are nominal typed immutable compile-time values. Compile time is
deterministic, budgeted, capability-limited, dependency-tracked. Never allow
string mixins/text-to-source; `eval`/source parsing/injection; ambient file/
network/process/clock/random/environment; attribute grammar/token changes;
unrestricted symbol/type enumeration; runtime get/set/call-by-name; compiler/
backend handles in runtime values.

Runtime reflection absent by default. Retained metadata is narrow serializable
projection consumed by generated typed adapters.

Generated code uses versioned backend-neutral PLRI. Pop GC: precise concurrent
generational, moving nursery, mostly non-moving mature heap, precise roots/stack
maps, safe points, SATB/generational barriers, bounded pause work. Do not add
finalizers, weak refs, resurrection, conservative scanning, untracked raw
managed pointers, unloading without accepted GC proof. Native/future VM preserve
same object/init/visibility/metadata/error/GC semantics.

## Libraries and artifacts

Exactly two reserved foundations:

- `Pop.Internal`: trusted compiler/runtime primitives, intrinsics, GC/ABI
  bridges, platform adapters; never user-referenced.
- `Pop.Standard`: native Pop portable public APIs and fixed curated prelude;
  depends on `Pop.Internal`, never inverse.

ADRs 0030/0031/0032 and public-standard-library architecture govern tiers,
naming, usability, costs, capability boundaries. Foreign mature libraries are
capability checklists, never Pop object/API templates. Common calls stay direct;
advanced typed options/views/buffers/streams/scopes expose allocation/dispatch.

`Pop.Data`, `Pop.Ai`, `Pop.Cli`, `Pop.Rpc`, `Pop.Syntax`, `Pop.Lsp` are normal
independently versioned Packages under ADR 0033; never fixed prelude or implicit
`Pop.Standard` dependencies.

Library Bubbles emit self-describing `.poplib`: `bubble.manifest`, public-only
`reference.metadata`, separate `documentation.xml`, target implementations,
hashes, ABI/capabilities, exact Bubble dependencies.

## Diagnostics, fixes, documentation

Diagnostics are structured semantic APIs: stable `POP####`, typed arguments,
spans/labels/notes/origins, intrinsic severity/category, warning waves,
suppression, semantic quick fixes. Compiler passes never emit final strings or
parse messages for facts; never disguise compiler/architecture incidents as
user errors; suppress errors/Lua regressions; suggest `Any`, dynamic lookup,
unsafe casts, reflection; auto-apply review/unsafe fixes; download/add
dependencies as ordinary unapproved source fix.

Safe fix-all is atomic, version-checked, composable, formatted, and verifies
postcondition. CLI/LSP/JSON/SARIF/tests render same diagnostic object.

XML docs use Lua-shaped `---` plus checked C#-inspired XML. Docs precede
attributes/declarations. Parse with DTD/entities/external resolution disabled.
Validate parameters/type parameters/returns/typed errors/effects/complexity/
allocation/thread safety/`cref`. `<code>` is docs/test input, never macro/string
mixin. Public `Pop.Standard` APIs require complete checked docs and compiled
nontrivial examples. Emit docs separately; never enable runtime reflection.

## Editing and validation

Preserve unrelated/user changes. Use focused `apply_patch`; no destructive reset
or broad mechanical rewrite without reason. Architecture docs remain English.
Use CommonMark/GitHub Markdown with blank lines around headings/lists. Examples
stay beautiful/minimal/static/Luau-shaped/canonical; wrong example is doc bug.
Synchronize links/terms after rename. External technical claims use official/
primary sources. Add no generated output, build/cache, credentials, editor files.

Avoid Python where possible; prefer Ruby. Repository scripts must not be Python:
if Ruby absent, ask host owner to install it and give OS instructions.

Architecture validation minimum:

- all relative Markdown links resolve;
- namespace examples honor default-internal and explicit public/private;
- no `export`, lowercase attributes, dynamic operation/type, canonical
  `Iter`/`iter.map`, arbitrary truncation;
- Item/Module/Bubble/Package/Workspace stays consistent;
- HIR/MIR remain backend-neutral;
- ADRs, closed decisions, roadmap, diagnostics, examples, conformance matrices
  agree.

Where implementation exists, run narrow relevant formatter, unit, conformance,
integration, cross-backend, architecture-regression suites. Never claim unrun
checks passed.

## Commits and completion

Linux commit style: concise specific imperative subject; no final period; blank
line before body; body explains why/problem/consequences rather than restating
code, wrapped about 72 columns. One logical change per independently
understandable/reviewable/revertible commit; keep working state when practical.
No vague `Update code`, `Fix stuff`, `Changes`, `WIP`.

Done only when request is implemented/documented; accepted architecture holds;
no contradictory old model remains; terminology/examples synchronize;
proportional verification passes; remaining gaps/unrun checks are stated.
