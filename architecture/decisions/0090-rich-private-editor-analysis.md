# ADR 0090: Rich Private Editor Analysis

- Status: accepted
- Date: 2026-07-16
- Supersedes: none
- Amends: ADR 0088, ADR 0089

## Context

ADR 0089 connected the private language server to compiler diagnostics,
checked-documentation hover, and declaration symbols. The compiler diagnostic
model already carries localized labels and notes plus versioned typed quick
fixes, but the adapter currently discards those facts. The compiler also has
verified HIR call targets and parameter declarations that can support useful
parameter-name inlay hints without textual symbol matching.

Analyzing every open file as an unrelated Bubble also produces incomplete
results for ordinary multi-Module Packages. Workspace and dependency loading
are not yet available as a reusable compiler query, so the next slice must
improve local Package awareness without pretending to implement the complete
Workspace resolver.

## Decision

The version-coupled private compiler projection may additionally expose:

- localized diagnostic labels and notes, stable category, warning wave,
  suppression identity, and fix IDs;
- source-only `WorkspaceEdit` values attached to compiler-produced quick fixes;
- parameter-name inlay hints for statically resolved direct calls when the
  compiler proves the target and parameter identity; and
- source-owned Modules belonging to the same conventionally discovered Bubble
  as the active document.

The LSP adapter maps labels to related diagnostic information, preserves
machine facts in diagnostic `data`, and publishes notes as localized text.
It exposes compiler fixes through `textDocument/codeAction`. A code action is
returned only when its diagnostic code and fix ID match the current immutable
document snapshot and all edits name known source files. The adapter binds the
private compiler edit plan to the analyzed LSP document version; it never
rebases an edit computed from an older snapshot. Safe fixes may be preferred;
review fixes are never marked preferred; unsafe fixes are not published by this
slice.

`textDocument/inlayHint` initially returns only parameter-name hints. Hints are
derived from typed direct-call dispatch and argument spans. The adapter omits
receiver parameters, ignored or absent names, indirect calls, unresolved code,
and a hint whose argument already uses an equally clear named form. Hints are
presentation only and never affect typing or evaluation.

For a file URI, the server walks ancestors and selects the nearest
`bubble.toml` containing `[package]`. That nearest Package boundary wins even
inside an outer Workspace; Package or Workspace membership never widens
visibility. It uses the accepted conventional discovery algorithm to find the
one Bubble owning the file and analyzes all source-owned Modules in that Bubble.
The first slice applies only when the selected Bubble has no unresolved
external or sibling-Bubble dependencies. Otherwise it retains explicit
standalone analysis and does not fabricate reference metadata. Untitled and
non-file documents are always standalone.

Filesystem discovery reads only `bubble.toml` and conventional `.pop` roots,
does not follow symlinks, does not scan `target`, dependency caches, or hidden
directories, and sorts normalized relative paths before assigning typed
session IDs. Paths select inputs but never become semantic identities.

Completion, signature help, cross-Bubble navigation/references, rename,
formatting, semantic tokens, incremental range edits, and full Workspace or
dependency analysis remain outside this decision.

## Consequences

- Errors, warnings, information, hints, related spans, and available compiler
  fixes reach editors without parsing rendered text.
- Common direct calls gain low-cost parameter guidance backed by verified HIR.
- Multi-Module dependency-free Packages receive coherent same-Bubble analysis;
  complex graphs remain explicitly incomplete rather than guessed.
- The compiler tooling projection grows, but private syntax trees, resolver
  databases, HIR, and MIR still do not escape compiler ownership.

## Alternatives considered

### Search source text for calls and declarations

Rejected because spelling cannot prove overload selection, shadowing, or
symbol identity.

### Run `pop check` for every workspace folder

Rejected because the language server must consume structured queries and
because process output is not a semantic API.

### Treat the outermost workspace folder as one project

Rejected because editor folders may contain unrelated or nested Packages and
must not merge Bubble visibility.

### Publish every compiler edit immediately

Rejected because stale, cross-file, review, or unsafe edits require stronger
versioning and atomic application contracts.

## Required conformance tests

- all four diagnostic severities preserve stable LSP severity and category;
- a secondary compiler label becomes related information at the exact UTF-16
  range and a note remains localized;
- a safe compiler quick fix becomes one version-matched code action and applies
  the exact edit;
- stale, unknown-file, review, and unsafe edits are rejected;
- direct-call parameter hints use compiler-selected parameter names and
  argument positions, while unresolved and indirect calls produce none;
- two Modules in one dependency-free Bubble resolve together;
- the nearest nested Package wins and a Workspace never merges Package
  visibility; and
- advertised capabilities match the implemented requests.

## Documents/components affected

Diagnostics architecture, CLI/tooling architecture, compiler driver tooling
projections, Package discovery, private language server, official editor
extensions, localization catalogs, and architecture conformance tests.
