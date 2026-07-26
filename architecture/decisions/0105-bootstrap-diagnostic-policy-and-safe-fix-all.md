# ADR 0105: Bootstrap Diagnostic Policy and Safe Fix-All

- Status: accepted
- Date: 2026-07-26
- Supersedes: none
- Amends: ADR 0010, ADR 0027

## Context

ADR 0010 requires warning waves, warning-as-error policy, a configurable
command-line error bound, and atomic safe fix-all. The structured diagnostic,
LSP, and source-edit types exist, but the bootstrap driver needs exact controls
and transaction behavior. Without those details, a renderer can accidentally
treat every warning as an error, machine adapters can omit policy/fix facts, and
an implementation can claim fix-all while applying edits incrementally.

## Decision

The bootstrap driver recognizes these repeatable global diagnostic controls
before command parsing:

```text
--warningWave <number|Latest>
--warningsAsErrors <*|WarningGroup|POP####>
--disabledWarnings <*|WarningGroup|POP####>
--maximumErrors <1..10000>
```

The 2026 edition default warning wave is `1`; `Latest` selects the highest wave
in the embedded diagnostic catalog. The default command-line maximum is `100`
effective errors. A promoted warning counts toward that bound but retains its
intrinsic `Warning` severity and code. Wave-disabled and explicitly disabled
warnings do not render or block. Disabling `*` affects warnings only; errors,
compiler incidents, and architecture regressions remain visible and blocking.

The structured report records total intrinsic warnings, effective errors,
omitted errors, promotion, and whether artifact publication is blocked.
Human, newline-delimited JSON, and LSP adapters preserve the same diagnostic
code, category, warning group/wave, suppression identity, origin facts, and fix
facts. Machine output includes fix applicability, stable equivalence identity,
workspace revision, and exact edits without requiring localized text parsing.
When the error bound omits errors, machine output emits a versioned
`diagnosticLimitReached` event and human output states the exact omitted count.

`pop fix <source.pop>` is the first source-only bootstrap surface for
unattended safe fix-all. It selects only `Safe` fixes whose provider supplied a
stable fix-all equivalence identity. It snapshots the source revision, sorts
and deduplicates edits, rejects stale/unknown/read-only/generated targets and
inconsistent overlap, applies to an in-memory candidate, reparses/rechecks the
candidate, and verifies the advertised diagnostics are removed. Publication
uses a same-directory temporary file and atomic rename; any failure before
rename leaves the original source byte-for-byte unchanged. Review and unsafe
fixes are reported as skipped and never applied by this command slice.

Multi-file Package/Workspace fix-all, changed-region formatting, interactive
review confirmation, and source-scoped suppression parsing remain required
follow-up work. They cannot weaken the snapshot, conflict, postcondition, or
atomic-publication contract.

## Consequences

- Warning policy changes build outcome without contaminating compiler semantic
  severity.
- Command output remains bounded and explains omitted diagnostics.
- Safe fix-all has one transactional implementation shared by future CLI and
  editor publication boundaries.
- The bootstrap source-only limitation is explicit rather than silently
  pretending to provide Workspace-wide atomicity.

## Alternatives considered

### Treat every diagnostic as a failed command

Rejected because warnings describe valid programs unless selected policy
promotes them.

### Make localized messages or TUI choices drive policy

Rejected because presentation is not semantic authority and machine adapters
must not scrape text.

### Apply each safe edit immediately

Rejected because a later conflict, stale version, parse failure, or failed
postcondition would leave a partially modified Workspace.

## Required conformance tests

- wave `0`, wave `1`, and `Latest` deterministically gate catalog warnings;
- group/code/all promotion blocks while preserving intrinsic warning severity;
- disabled-warning selectors cannot suppress an error;
- the effective-error bound emits the exact omitted count;
- JSON and LSP preserve warning group/wave, suppression, applicability,
  equivalence, revision, and edit facts;
- safe fix-all composes deterministic non-overlapping edits;
- stale, read-only, invalid, and conflicting edits leave every source
  unchanged;
- failed reanalysis/postcondition leaves every source unchanged;
- review, unsafe, and safe-but-unproven fixes are excluded from unattended
  fix-all; and
- source publication is atomic and a second fix run is idempotent.

## Documents/components affected

- `architecture/17-diagnostics-warnings-and-quick-fixes.md`
- `architecture/21-cli-tooling-and-code-units.md`
- `architecture/19-architecture-conformance-and-regression-policy.md`
- `crates/compiler/diagnostics`
- `crates/compiler/driver`
- `crates/tools/language-server`
