# Diagnostics, Warnings, and Quick Fixes

## Goals

Pop Lang diagnostics are a compiler API, editor API, and user experience—not
formatted strings printed as an afterthought.

Every diagnostic should answer:

1. What is wrong or risky?
2. Where is the primary cause?
3. Why did the compiler reach this conclusion?
4. What concrete action can correct it?
5. Can the compiler apply that correction safely?

Diagnostics must remain useful in terminals, IDEs, build servers, generated
code, compile-time execution, and multi-Bubble Workspace builds.

## Structured diagnostic model

Conceptual representation:

```text
Diagnostic
  code: DiagnosticCode
  severity: DiagnosticSeverity
  category: DiagnosticCategory
  messageKey: MessageKey
  arguments: DiagnosticArguments
  primarySpan: SourceSpan
  labels: List<DiagnosticLabel>
  notes: List<DiagnosticNote>
  originChain: List<DiagnosticOrigin>
  fixes: List<QuickFix>
  warningWave: WarningWave?
  suppressionKey: SuppressionKey?
```

Messages are rendered from typed arguments. Compiler passes do not build final
English sentences or parse messages to discover facts later.

Human messages use the private toolchain localization contract in
[ADR 0088](./decisions/0088-localized-toolchain-presentation.md). English is the
canonical catalog schema, not a sentence embedded in a compiler pass. Every
argument has a stable name and closed value kind so all official translations
can be checked for exact placeholder parity.

Catalog ownership mirrors the toolchain: compiler lexer, parser, resolution,
types, compile-time, documentation, FFI, backend, CLI, LSP, and shared
presentation text use separate formatted TOML fragments below each locale.
Fragment boundaries organize ownership only; message keys remain globally
unique and the aggregate locale is validated atomically.

### Severity

`DiagnosticSeverity` has:

- `Error`: the requested artifact cannot be produced correctly;
- `Warning`: the program is valid but likely incorrect, fragile, unsafe, or
  unexpectedly expensive;
- `Information`: relevant build/analysis information without a likely defect;
- `Hint`: low-noise editor guidance, normally hidden in command-line output.

Promoting a warning to an error changes build policy, not the diagnostic's
intrinsic severity or code. Suppressions cannot hide errors or compiler bugs.

### Categories and code ranges

Built-in diagnostic IDs are stable uppercase codes:

| Range | Category |
| --- | --- |
| `POP0001–POP0999` | Lexing, parsing, syntax, formatting |
| `POP1000–POP1999` | Namespaces, symbols, visibility, libraries, loading |
| `POP2000–POP2999` | Types, inference, generics, overloads, conversions |
| `POP3000–POP3999` | Flow, initialization, effects, results, coroutines |
| `POP4000–POP4999` | UDAs, compile-time evaluation, metadata capabilities |
| `POP5000–POP5999` | Unsafe code, FFI, GC correctness, concurrency |
| `POP6000–POP6999` | Style, API design, performance, maintainability warnings |
| `POP7000–POP7999` | MIR, backend, linker, artifact, target capability |
| `POP8000–POP8999` | Package/project configuration and reproducibility |
| `POP9000–POP9999` | Reserved toolchain integration diagnostics |

An internal compiler failure uses a separate incident identifier and “compiler
bug” channel. It is not disguised as a normal `POP` source error.

Architecture drift/Lua regression detected by verifier or conformance tests is a
toolchain bug and release failure, never a suppressible user warning. User code
should not be blamed for violating an invariant the compiler promised to enforce.

Third-party analyzers use a registered vendor prefix rather than claiming `POP`
codes. Code identity is stable even when wording improves.

### Visibility and complete-name enforcement

The initial catalog includes structured diagnostics for:

- a namespace-scope declaration missing `public`, `internal`, or `private`
  (`Error`);
- use of the unsupported `export` draft syntax (`Error`) with a `public`
  migration fix;
- access to an `internal` declaration from another Bubble or a `private`
  declaration from another Module (`Error`);
- an arbitrary public-name truncation such as `Iter` (`Style`/`ApiDesign`), with
  context-aware `Iterable`, `Iterator`, or `Sequence` fixes;
- attempts to model namespace functions through a stateless utility class
  (`ApiDesign`).

The base libraries enable the complete-name and non-OOP API diagnostics as
errors. The exact `Iter`/`iter.map` standard surface is rejected by API baseline
tests rather than grandfathered as an abbreviation.

## Rendering

Human output follows a compact Rust-quality/Luau-friendly layout without visual
noise:

```text
error[POP2001]: `nil` is not assignable to `String`
  --> source/player.pop:8:24
   |
 8 | local displayName: String = nil
   |                    ------   ^^^ expected `String`, found `nil`
   |
help: use an optional type: `String?`
```

Rules:

- the first line is a complete short statement;
- the primary underline points to the smallest responsible span;
- secondary labels explain related declarations/constraints;
- notes present causal context, not generic documentation;
- help describes a concrete action and corresponds to a quick fix when possible;
- paths are workspace-relative by default;
- color is optional and never carries meaning alone;
- generated/desugared code maps back through source-origin chains.

The renderer receives an immutable locale context. The CLI, language server,
tests, and future SARIF adapter share message lookup and typed substitution;
semantic queries never read environment or user configuration. The initial
official human languages are `en`, `zh-Hans`, `ja`, `pt-BR`, and `es`.

Diagnostic codes, identifiers, type names, paths, package identities, source
text, and target triples are never translated. Labels such as “error”, “note”,
and “help” are presentation text and are translated. JSON and protocol facts
stay locale invariant; translated display text is never required to recover a
code, span, argument, edit, or origin.

## Diagnostic production

Each compiler subsystem returns diagnostics through a shared sink/query result:

- parser recovery emits syntax diagnostics and valid recovery nodes;
- resolver reports unknown/ambiguous/inaccessible symbols and using/Bubble
  context;
- type checker reports constraint conflicts with reason chains;
- flow/effect analysis reports paths and invalidation points;
- compile-time execution reports the call/dependency chain and UDA attachment;
- MIR/backend verifier reports an internal compiler bug unless invalid external
  MIR was explicitly supplied;
- loader/linker reports dependency identity and resolution paths.

No pass writes directly to stderr. The driver sorts and renders after query
results stabilize.

## Error recovery and cascade control

The compiler continues after source errors to support IDE feedback, but recovery
must not create dozens of fictional errors.

- Syntax recovery inserts explicit missing/error nodes with bounded reach.
- `ErrorType` satisfies follow-on constraints without becoming valid HIR.
- A diagnostic records root-cause IDs that poisoned dependent analysis.
- Duplicate diagnostics at the same semantic cause coalesce.
- Dependent module errors summarize the failed dependency once rather than
  replaying its full diagnostics at every use.
- The command line defaults to a configurable maximum error count; IDE queries
  prioritize the active file without changing semantic results.
- Successful declarations continue to produce navigation/completion data.

## Type diagnostic explanations

Constraint failures retain a reason graph. Rendering selects a small causal path:

```text
error[POP2007]: `Player?` cannot be passed as `Player`
  --> source/match.pop:14:17
   |
14 | startMatch(player)
   |            ^^^^^^ this value may be `nil`
   |
note: `findPlayer` returns `Player?`
  --> source/players.pop:22:45
help: check for `nil` before this call
```

Full solver graphs remain available in machine/debug output, not dumped into the
normal message.

ADR 0095 reserves `POP2032` for a checked-cast target that is not one fully
applied named class, `POP2033` for an operand that is not one non-optional
nominal interface value, and `POP2034` for a target without the exact nominal
source-interface implementation. These diagnostics carry typed source/target
identities and spans where available. No automatic fix changes the target,
inserts an unchecked assertion, enables reflection, or silently unwraps the
optional result.

## Warning system

### Warning groups

Warnings belong to stable named groups:

- `Correctness`;
- `Nullability`;
- `Concurrency`;
- `Unsafe`;
- `Performance`;
- `Allocation`;
- `ApiDesign`;
- `Documentation`;
- `Style`;
- `Compatibility`;
- `Deprecated`;
- `Unused`.

Group names are PascalCase because they are enum-like identifiers. Projects can
configure groups or individual IDs.

`ApiDesign` specifically reinforces Pop Lang's non-OOP default: it can flag
data-only/stateless classes, unnecessary inheritance, redundant contextual type
names, marker interfaces, and service/factory/helper class ceremony. These are
review warnings, not bans; see
[Paradigm and API style](./18-paradigm-and-api-style.md).

### Warning waves

New warnings that would appear in previously accepted code enter a numbered
warning wave. The selected language/toolchain edition enables a stable default
wave; developers can opt into `Latest` before upgrading.

Conceptual project configuration:

```text
diagnostics:
  warningWave: 3
  warningsAsErrors: ["Correctness", "POP5004"]
  disabledWarnings: ["POP6012"]
```

- Existing errors are never delayed by warning waves.
- A warning promoted to error remains suppressible as a warning policy decision.
- CI can use `warningsAsErrors: ["*"]` without automatically enabling future
  waves.
- Security/correctness warnings may become default errors only in a language
  edition with migration notes and fixes.
- Standard-library builds use `Latest` and treat all enabled warnings as errors.

### Scoped suppression

Warnings can be suppressed by exact ID or group at project or declaration/file
scope. Source suppression uses a PascalCase compiler attribute:

```luau
@SuppressWarning("POP6012", reason = "Protocol field required by external ABI")
public record LegacyPacket
end
```

Rules:

- a reason is required in checked-in source;
- suppress the narrowest declaration/span supported;
- errors and compiler incidents cannot be suppressed;
- unknown/retired diagnostic IDs produce a diagnostic;
- suppression is preserved in HIR origin metadata for audit tooling;
- generated code suppressions do not leak into user-written source;
- a project can reject source suppressions for selected groups;
- blanket “all warnings” source suppression is not allowed.

For migration, a baseline file stores diagnostic code plus semantic location/
fingerprint. Baselines suppress only existing matches and report stale entries.

## Quick-fix model

```text
QuickFix
  fixId: QuickFixId
  diagnosticCode: DiagnosticCode
  titleKey: MessageKey
  applicability: FixApplicability
  equivalenceKey: FixEquivalenceKey?
  edits: WorkspaceEdit
  postCondition: FixPostCondition
```

`FixApplicability` is:

- `Safe`: semantics are preserved or the intended correction is uniquely proved;
- `RequiresReview`: likely correction with a meaningful behavior choice;
- `Unsafe`: potentially destructive; never auto-applied or included in fix-all.

Quick fixes consume typed semantic facts from the diagnostic. They never scrape
rendered messages.

### Initial fixes

- add or correct a type annotation;
- change `T` to `T?` when that is a valid explicit choice;
- insert a proven `nil` check/narrowing branch;
- add a `using` directive for an already referenced unambiguous namespace;
- add a missing `public`, `internal`, or `private` modifier to a namespace-scope
  declaration when its intended accessibility is uniquely known;
- replace the obsolete draft `export` prefix with `public` as a migration fix;
- qualify an ambiguous symbol;
- rename an identifier to PascalCase/camelCase/UPPER_SNAKE_CASE;
- expand arbitrary truncations such as `Iter` to `Iterable` or `Iterator`, using
  resolved symbol/type context; ambiguous expansions require review;
- create missing nominal interface members;
- add missing exhaustive match cases;
- add a missing `return`, result propagation, or `await` where uniquely valid;
- convert table-shaped data into a record plus functions; offer a class only
  when identity/lifecycle requirements are explicit;
- convert a stateless/data-only class into a record/namespace-function design;
- move stateless functions directly into their namespace instead of generating
  a utility class or singleton object;
- replace a deprecated standard-library API with its declared successor;
- add `@SuppressWarning` only as an explicit secondary action, never the primary
  fix for correctness warnings.
- insert/repair checked XML documentation summaries, parameters, returns,
  errors/effects, and symbol references.

A quick fix never introduces `Any`, dynamic lookup, unrestricted reflection,
string mixins, or an unsafe cast merely to silence the type checker.

### Workspace edits

A `WorkspaceEdit` contains versioned text edits, file creates/renames, and an
optional `bubble.toml` edit. Applying it requires:

- the source versions used to compute it still match;
- edits do not overlap inconsistently;
- read-only/generated/vendor files are not modified;
- the formatter runs only on changed syntax regions;
- the result parses, and safe fixes satisfy their advertised postcondition;
- multi-file edits apply atomically or not at all.

Standard-library names normally need no using fix because the fixed `Pop` prelude
is already open. Adding `using Studio.Shared` is a source-only safe edit when that
external Bubble is already referenced. Adding/downloading a new Package is a
separate dependency action with preview and user approval, not an ordinary quick
fix.

Dependency actions are implemented by `pop add`/`pop remove`; editor code
actions call the same transactional service and show Package, Bubble, version,
source, feature, lockfile, and license changes before approval.

### Fix all

A provider can opt into fix-all only with a stable equivalence key and proof
that edits compose. Supported scopes are declaration, file, namespace, project,
and workspace.

The engine:

1. snapshots diagnostics and document versions;
2. groups by provider/equivalence key;
3. computes semantic edits from the snapshot;
4. detects conflicts and dependency ordering;
5. applies an atomic workspace edit;
6. reparses/rechecks affected queries;
7. reports skipped conflicts rather than guessing.

`RequiresReview` fixes show a preview and are excluded from unattended fix-all.
`Unsafe` fixes never support fix-all.

## Compiler and analyzer providers

Built-in diagnostics and fixes use the same provider interfaces as official
analyzers, but compiler correctness does not depend on loading a plugin.

Conceptual contracts:

```text
DiagnosticProvider
  supportedCodes() -> List<DiagnosticCode>
  analyze(context: AnalysisContext) -> List<Diagnostic>

QuickFixProvider
  fixableCodes() -> List<DiagnosticCode>
  provideFixes(context: QuickFixContext) -> List<QuickFix>
  provideFixAll() -> FixAllProvider?
```

Third-party analyzers execute through a sandboxed/versioned compiler API or
separate process. They receive typed read-only semantic models, cannot mutate the
compiler, and declare performance/capability requirements.

UDA processors can emit structured diagnostics with source origins, but cannot
claim built-in `POP` codes.

## Output formats

The same diagnostic object renders to:

- human terminal text;
- compact/verbose JSON with schema version;
- Language Server Protocol diagnostics and code actions;
- SARIF for CI/security tooling;
- test snapshot format with normalized paths;
- optional binary incremental transport between compiler daemon and editor.

Machine output includes stable codes, severity, category, spans, related spans,
typed/string-rendered arguments, suppression state, warning wave, origin chain,
and fix IDs. Localized human text is not a machine contract.

## Ordering and determinism

Diagnostics sort by:

1. Package/Bubble/Module order from the deterministic build graph;
2. normalized file path;
3. primary start/end span;
4. severity;
5. diagnostic code;
6. stable origin tiebreaker.

Parallel compilation cannot change order, deduplication, or which diagnostic is
chosen as the root cause.

## Performance and cancellation

- Diagnostics needed for typing are produced with their owning compiler query.
- Optional analyzers have budgets and cancellation.
- Quick fixes compute lazily when requested or precompute only cheap edit plans.
- Expensive project-wide fixes run with progress and cancellation.
- The active editor document receives priority without starving build analysis.
- A cancelled provider publishes no partial fix/edit.

## Diagnostic catalog

The repository maintains one machine-readable catalog as the source of truth for:

- code and default severity/category;
- warning wave;
- message/help keys and typed argument schema;
- suppressibility;
- documentation URL;
- owning compiler component;
- registered quick-fix providers;
- edition introduction/deprecation.

Code generation creates typed diagnostic constructors so passes cannot use the
wrong arguments. CI rejects duplicate codes, missing docs, invalid ranges, and
orphan fixes.

## Testing requirements

- golden human-rendering tests;
- exact localization-key and named-placeholder parity for every official
  toolchain catalog;
- independent CLI and language-server locale-selection tests;
- JSON/LSP/SARIF schema tests;
- source-recovery and cascade-count tests;
- warning-wave/edition matrix tests;
- suppression scope, reason, and stale-baseline tests;
- quick-fix preview/apply/postcondition tests;
- fix-all conflict and determinism tests;
- formatting/idempotence after edits;
- localized message argument tests;
- parallel ordering tests;
- fuzzing malformed source, spans, edits, and analyzer output;
- latency budgets for active-file diagnostics and cheap fixes.

## C# and Roslyn influence boundary

C# warning waves demonstrate how new warnings can be introduced without
surprising existing builds, while Roslyn's code-fix model ties fixes to stable
diagnostic IDs and optionally supports fix-all. Pop Lang adopts these principles
with its own typed diagnostic model, Luau-like source presentation, and stricter
safe-edit/postcondition requirements.

Primary references:

- [C# compiler warning waves](https://learn.microsoft.com/en-us/dotnet/csharp/language-reference/compiler-messages/warning-waves)
- [Roslyn CodeFixProvider](https://learn.microsoft.com/en-us/dotnet/api/microsoft.codeanalysis.codefixes.codefixprovider)
