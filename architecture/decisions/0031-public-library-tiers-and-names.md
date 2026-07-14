# ADR 0031: Public Library Tiers and Names

- Status: accepted
- Date: 2026-07-12
- Depends on: ADR 0030
- Clarified by: ADR 0032 concise APIs and explicit cost contracts
- Superseded in part by: ADR 0058 exact standard-foundation prelude

## Context

A broad library needs discoverable names without turning `Pop` into a flat
prelude or making every program depend on platform frameworks.

## Decision

`Pop` remains the root namespace and `Pop.Standard` remains the automatically
referenced foundation Bubble. `Pop.Standard.Core` is an implementation
partition, not another public Bubble or source namespace. ADR 0058 supersedes
this record's illustrative prelude contents with one exact baseline. Optional
values use `T?`, not a duplicate nominal `Option<T>` wrapper, and `Sequence` is
the sole initial namespace root. Members remain qualified.

Names describe a stable capability, not a framework role. Common formats are
the direct roots `Json`, `Yaml`, `Xml`, `Csv`, and `Toml`; shared schema and
binary-codec work lives under `Codec`. Regular expressions and globs are
`Regex` and `Glob`, not a vague pattern hierarchy. `Locale` covers locale data
and typed messages. `Terminal` owns terminal behavior. `Telemetry` owns logs,
traces, and metrics. Broad system state is split into `Process`, `Environment`,
and `Platform`. Timers remain in `Time`; database transactions remain in `Sql`.
`System`, `Runtime`, `Env`, `Context`, `Observe`, `Term`, `Timers`, and
`Transactions` are not public catch-all roots. ADR 0033 accepts `Data` as the
focused root of the separately installed `Pop.Data` extension.

Application and specialist families use direct roots such as `Data`, `Cli`,
`Http`, `Sql`, `Image`, `Graphics`, `Audio`, `Video`, `Ui`, `Science`, `Ai`,
`Syntax`, and `Lsp`. These are
official Packages unless section 22 assigns a narrow value contract to
`Pop.Standard`. Their direct names do not make them implicit dependencies.

Types, attributes, domains, capabilities, and package identities are
`PascalCase`; functions and Modules are `camelCase`; constants are
`UPPER_SNAKE_CASE`. `Unsafe` and `Native` are explicit final namespace segments.
Experimental APIs use a separate `Pop.Experimental.*` Package/Bubble identity
and never enter the normal prelude.

The catalog explicitly accepts these established technical forms, cased as
words: `Ai`, `Api`, `Cli`, `Csv`, `Crypto`, `Ffi`, `Gpu`, `Guid`, `Http`, `Io`,
`Ipc`, `Json`, `Lsp`, `Mime`, `Rpc`, `Sql`, `Tls`, `Toml`, `Ui`, `Uri`, `Usb`,
`Utf8`, `Xml`, and `Yaml`. This is a closed reviewed vocabulary, not permission
for arbitrary project abbreviations.

## Rationale

Short qualified names make APIs discoverable without a huge global namespace.
Grouping formats, resource localization, terminal behavior, and system facts by
their real contracts avoids overlapping catch-all roots and makes portability
and capability requirements visible at the call site.

## Consequences

- Common calls stay shallow: `Json.decode`, `Regex.find`, `File.read`,
  `Http.send`, `Sql.query`, and `Telemetry.log`.
- Names such as `Client`, `Builder`, `Provider`, `Manager`, and `Service` need a
  resource-state justification; otherwise an operation namespace/configuration
  record replaces them.
- Naming and prelude changes are versioned public API changes.

## Compatibility impact

No planned name is an implementation claim. A compatibility alias is allowed
only when it is source-safe, does not widen the fixed prelude, and has a
documented removal edition. No alias preserves a deep or OOP-shaped API solely
for familiarity.

## Security and portability impact

`Unsafe`, `Native`, `Platform`, and `Experimental` visibly partition APIs whose
availability, trust, or stability differs from portable `Pop.Standard`. A
qualified namespace never substitutes for a typed capability requirement.

## Implementation impact

The package resolver, API baseline, documentation generator, formatter, and
language server must expose tier, availability, stability, and unsafe metadata.
No implementation may reserve a namespace simply because it appears in the
catalog.

## Alternatives considered

### Preserve the first-draft names

Rejected because `Data.Json`, `Text.Pattern`, `Term`, `Observe`, `System`,
`Component`, `Runtime`, `Env`, and `Context` add depth, truncate words, or hide
distinct contracts in vague domains.

### Put every format below one data namespace

Rejected because the extra `Data` segment repeats information at common call
sites. Shared schema, streaming, safety, and generated-adapter contracts do not
require every format to share a source namespace.

## Migration

New public APIs use these names immediately. Existing bootstrap APIs are
reviewed one domain at a time; deprecations include an automated rewrite only
when the semantic replacement is exact.

## Accepted technical forms

`Ui` and `Ai` are accepted technical forms under this ADR. Their brevity is
domain-standard and improves qualified call sites without creating ambiguous
arbitrary truncation.

## Resolved questions

ADR 0058 fixes the first prelude and its sole namespace root, `Sequence`.

## Required conformance tests

- exact root/tier/status inventory and duplicate-root rejection;
- exactly one domain catalog owner for every root;
- prelude snapshot, collision, and explicit-import tests;
- rejection of stale first-draft roots and unreviewed abbreviations;
- Package dependency tests proving optional/platform families remain explicit.

## Documents/components affected

Architecture sections 13, 16, 18, 21, 22, bootstrap metadata, API baselines,
documentation, package tooling, and migration diagnostics.
