# ADR 0027: Ratatui Terminal Presentation

- Status: accepted
- Date: 2026-07-11
- Amends: ADR 0018 if accepted
- Relates to: ADR 0010, ADR 0017

## Context

ADR 0017 establishes one `pop` command with consistent selection, diagnostics,
configuration, and machine protocols. ADR 0010 requires human, JSON, LSP, SARIF,
and test presentation to render from the same structured diagnostic model. The
integrated CLI architecture also requires optional color that never carries
meaning and forbids tools from scraping human output.

Some explicitly interactive workflows benefit from a terminal presentation for
selection, progress, previews, and confirmation. These include choosing among
multiple Bubbles and reviewing structured workspace edits. The ordinary command
path must remain predictable for shells, CI, editors, redirected streams, and
assistive tools. A terminal UI therefore cannot become an implicit execution
mode or a second semantic implementation of CLI behavior.

ADR 0018 currently permits only the Rust standard library in the initial
skeleton plus the separately approved Inkwell exception. Ratatui and its
Crossterm backend require an explicit dependency decision and confinement proof
before they can enter the accepted workspace.

## Decision

If this proposal is accepted, the Pop Lang toolchain approves Ratatui exactly at
version `0.30.2` for terminal presentation. The dependency declaration disables
default features and enables only the Crossterm 0.29 backend feature:

```toml
ratatui = { version = "=0.30.2", default-features = false, features = ["crossterm_0_29"] }
```

No Termion, Termwiz, Termina, unstable, calendar, palette, serialization, macro,
or other optional Ratatui feature is approved by this decision. Crossterm is
consumed through the Ratatui-selected backend so the process has one compatible
terminal event/raw-mode implementation.

Ratatui and Crossterm types are confined to the `pop` presentation and
orchestration boundary. They do not appear in project discovery, compiler
queries, diagnostics, workspace edits, HIR, MIR, backends, runtime interfaces,
library APIs, artifact schemas, or machine protocols. Presentation consumes
immutable structured command state, diagnostics, build events, selections, and
edit previews through backend-neutral internal interfaces.

This proposal does not change Pop Lang source syntax, language semantics,
Item/Module/Bubble/Package/Workspace ownership, backend behavior, or artifact
identity.

### Explicit activation

Ordinary `pop` commands never start a terminal UI automatically. Interactive
presentation is requested only with the complete long option `--interactive` on
a command whose accepted contract supports interaction.

Interactive mode requires both a terminal-capable input stream and a
terminal-capable presentation stream. For the current command surface these are
standard input and standard error, and both must report that they are TTYs;
standard output remains available for command, program, dump, and machine data.
If either required stream is unavailable:

- read-only commands use the deterministic plain renderer and continue;
- commands that require a user choice emit the same plain preview, perform no
  mutation, and fail with a structured user-action-required diagnostic;
- the tool never guesses a selection, interprets end-of-file as approval, or
  opens `/dev/tty` or another ambient terminal behind redirected streams.

The fallback is part of the tested command contract. It is not an error to ask
for interactive presentation in a non-terminal environment when the command can
complete without a choice.

### Machine output and deterministic plain rendering

`--messageFormat json` always bypasses Ratatui, raw mode, alternate-screen mode,
interactive prompts, cursor control, and ANSI styling. Machine output retains
its versioned schema and deterministic ordering. A request combining JSON with
an operation that needs confirmation emits a structured refusal and performs no
mutation.

The existing plain human renderer remains the canonical non-interactive
presentation. It renders the same structured objects as the TUI, uses
workspace-relative paths, and has deterministic content and ordering independent
of terminal size, event timing, or thread scheduling. Ratatui is never used to
produce snapshot, JSON, LSP, SARIF, or debug-dump formats.

### Color and accessibility

The shared color policy is `--color auto|always|never`. An explicit `--color`
option takes precedence over the `NO_COLOR` environment variable. When no
explicit option is present, a present and non-empty `NO_COLOR` selects `never`;
otherwise `auto` enables color only for a capable terminal presentation stream.
JSON and other machine formats always behave as `never`.

Color, animation, cursor position, borders, symbols, and text attributes never
carry information alone. Every state, severity, focus, selection, progress
result, and confirmation choice has a textual label. The interface is fully
keyboard operable, exposes visible focus, avoids time-limited confirmation, and
does not require mouse input. Layouts that cannot present their controls and
labels at the current terminal size switch to the deterministic plain renderer
rather than truncating a decision or diagnostic.

Animations are optional decoration and are disabled when they would obscure
stable progress or make cancellation less responsive. Source excerpts and edit
previews preserve text content when color and Unicode border glyphs are removed.

### Cancellation, errors, and terminal restoration

Entering interactive mode establishes one terminal-session guard. Every normal
return, structured error, I/O failure, cancellation, and caught unwind restores
raw mode, cursor visibility, mouse capture, and the original screen before
rendering the final plain diagnostic or returning control to the shell.

The process installs a scoped panic hook that attempts restoration without
panicking, then invokes the previous hook so the original compiler-bug report is
preserved. Cleanup is idempotent. Catchable termination and cancellation paths
use the same restoration routine. Terminal cleanup failure is reported after the
original failure and never replaces it.

Cancellation publishes no partial workspace edit, lockfile update, source
mutation, or cache entry. Rendering may show progress already completed, but it
does not redefine the transactional boundaries of the underlying operation.

### Fix and dependency-action confirmation

The TUI consumes the structured applicability and workspace-edit model from ADR
0010. It never parses rendered help or diagnostic text to discover an action.

- `Safe` fixes may participate in unattended safe fix-all under the existing
  atomicity, version, formatting, and postcondition rules. Interactive mode
  presents the complete edit set before an optional batch confirmation.
- `RequiresReview` fixes are excluded from unattended fix-all. Interactive mode
  shows the affected files, semantic consequence, and conflicts, then requires
  an affirmative confirmation for each equivalence group before application.
- `Unsafe` fixes are never auto-applied and never participate in fix-all. If an
  accepted provider exposes manual application, the user must deliberately
  select the individual fix, view its complete preview, and confirm it in a
  separate step. Cancellation or fallback to a non-terminal performs no edit.

Dependency additions, removals, downloads, and lockfile changes remain separate
previewed actions. The TUI must show Package, Bubble, version, source, feature,
license, and lockfile consequences before approval; it cannot disguise a
dependency action as a source quick fix.

### Dependency and security boundary

Acceptance requires updating the ADR 0018 dependency allowlist and architecture
tests to prove all of the following:

- only the presentation/orchestration boundary declares Ratatui;
- no semantic, backend, runtime, library, or machine-schema crate depends on it
  directly or transitively through a Pop Lang workspace crate;
- the exact Ratatui version, disabled defaults, and sole Crossterm backend
  feature remain pinned centrally;
- a dependency graph test rejects a second semver-incompatible Crossterm event or
  raw-mode implementation;
- Ratatui's MIT license and every enabled transitive dependency license are
  compatible with the repository license policy and recorded in the dependency
  review;
- the locked source and checksums are reproducible, known advisories are reviewed,
  and dependency updates repeat the license, feature, and vulnerability review;
- no optional network, serialization, macro, image, clipboard, shell, or plugin
  capability enters through the terminal dependency graph.

Ratatui is a renderer and input adapter, not a compiler plugin or capability
boundary. Terminal events cannot mutate compiler state except through the same
typed, validated command requests used by non-interactive execution.

## Consequences

- Interactive selection and previews can provide a richer terminal experience
  without changing command semantics or machine protocols.
- CI, editors, redirected commands, and users who avoid terminal UIs retain a
  complete deterministic plain interface.
- The driver gains an additional reviewed dependency graph and terminal-lifecycle
  responsibility.
- Ratatui upgrades are deliberate architecture-visible dependency changes rather
  than implicit compatible-version updates.
- A TUI failure cannot authorize an edit, change diagnostic identity, or leave
  semantic work partially committed.

## Alternatives considered

### Start interactive mode whenever `pop` has a terminal

Rejected because command behavior would vary implicitly between terminals, CI,
editors, and redirected streams. It would also make stable scripting and failure
reproduction harder.

### Replace plain human output with Ratatui rendering

Rejected because the plain renderer is required for accessibility, logs,
snapshots, redirection, and environments without terminal control support.

### Enable Ratatui default features

Rejected because the defaults select a broader dependency and feature surface
than Pop Lang needs. The backend and every optional capability must remain
explicit and reviewable.

### Let the terminal layer own diagnostics or edits

Rejected because ADR 0010 requires all presentations to consume one structured
diagnostic and workspace-edit model. Presentation text is not a semantic API.

## Required conformance tests

- ordinary commands never initialize Ratatui without `--interactive`;
- read-only `--interactive` commands fall back deterministically when either
  required stream is not a terminal;
- mutation requiring confirmation makes no change under non-terminal fallback,
  end-of-file, cancellation, or invalid input;
- JSON bypasses Ratatui and ANSI control sequences and remains schema-identical
  across terminal/non-terminal execution;
- `--color` precedence, `NO_COLOR`, automatic terminal detection, and colorless
  semantic equivalence are covered as a matrix;
- the plain renderer is deterministic across terminal sizes, event timing, and
  parallel build schedules;
- every interactive state and action remains understandable without color,
  borders, animation, Unicode glyphs, or mouse input;
- resize below the minimum usable layout falls back without losing diagnostic or
  confirmation content;
- normal exit, command error, input/output error, cancellation, and panic restore
  terminal state exactly once and preserve the original failure;
- Ratatui's in-memory `TestBackend` covers rendering, focus movement, selection,
  resize, confirmation, and cancellation without a real terminal;
- safe, review, and unsafe fix applicability follows the confirmation and fix-all
  rules above, including conflict and atomic-postcondition tests;
- dependency actions cannot execute through the ordinary source-fix path;
- architecture tests enforce the exact dependency/version/features, one
  Crossterm implementation, and confinement to presentation/orchestration;
- human plain/TUI and JSON presentations originate from equal structured command,
  diagnostic, build-event, and edit data.

## References

- [Ratatui 0.30.2 installation](https://ratatui.rs/installation/)
- [Ratatui 0.30.2 API documentation](https://docs.rs/ratatui/0.30.2/ratatui/)
- [Ratatui 0.30.2 feature flags](https://docs.rs/crate/ratatui/0.30.2/features)
- [Ratatui backend concepts and `TestBackend`](https://ratatui.rs/concepts/backends/)
- [Ratatui panic-hook restoration guidance](https://ratatui.rs/recipes/apps/panic-hooks/)
- [Ratatui source repository and MIT license](https://github.com/ratatui/ratatui)

## Documents/components affected if accepted

ADR 0018, CLI/tooling architecture, diagnostic rendering and fixes, closed
toolchain decisions, implementation roadmap, root Cargo dependency policy,
architecture dependency tests, `pop` driver presentation, terminal lifecycle,
and CLI conformance tests.
