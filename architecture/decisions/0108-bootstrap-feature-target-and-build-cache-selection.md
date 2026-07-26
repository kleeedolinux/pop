# ADR 0108: Bootstrap Feature, Target, and Build-Cache Selection

- Status: accepted
- Date: 2026-07-26
- Supersedes: none
- Amends: ADR 0055, architecture 21

## Context

The Package architecture requires additive features, an exact platform target,
one canonical lock graph, and checkout-location-independent cache keys. The
bootstrap manifest parser already distinguishes normal, development, and exact
platform dependency scopes, but no accepted syntax selects optional
dependencies and the unified CLI still silently chooses its only native target.
The shared Workspace `target/` root is also only an output directory: unchanged
builds repeat lowering, object emission, and linking without a verified reuse
record.

Copying Cargo's entire feature grammar would add aliases and conditional source
behavior that Pop Lang has not accepted. Treating output existence or timestamps
as a cache hit would make stale artifacts authoritative.

## Decision

### Bootstrap features

One Package may declare named additive features:

```toml
[features]
http = ["dependency:Http"]
telemetry = ["dependency:Telemetry"]

[dependencies]
Http = { version = "1.0.0", optional = true }
Telemetry = { path = "../telemetry", version = "1.0.0", optional = true }
```

Feature names are unique `camelCase` identifiers. Each value is a bounded,
duplicate-free string array. Schema 1 accepts only `dependency:<Alias>`
members. The alias must name an optional normal dependency in the same
manifest, and every optional dependency must be enabled by at least one
feature. Feature cycles, feature-to-feature forwarding, default features,
negative features, conditional source, and target/resource gating remain
outside this bootstrap decision.

The CLI selects features through repeatable `--feature <camelCase>` options on
manifest-driven check, build, run, test, benchmark, documentation, and lint
commands. Selection is additive and order-independent; unknown or duplicate
names fail before resolution. Optional dependencies enter only when enabled.
Development and exact-platform scope rules continue to apply independently.

The exact sorted selected feature set is stored on each selected root Package
record in `bubble.lock`, contributes to Bubble/cache identity, and is visible in
structured command inputs. A lock mismatch under `--locked` or `--frozen`
fails normally.

### Platform target

Manifest-driven compiler commands accept one exact
`--platformTarget <triple>`. Omission selects the toolchain's declared host
native target; the bootstrap toolchain currently declares
`x86_64-unknown-linux-gnu`. The selected target controls platform dependency,
FFI generator, native-link-plan, lock, artifact, and cache selection.

An unknown target is rejected before analysis. A known target whose required
command/backend/link capability is absent receives a clear capability error;
it never falls back to the host. The existing explicit BPF build form remains a
separate capability/profile contract and is not treated as an ordinary native
Package build.

### Verified build reuse

Every native Package/Workspace build computes a canonical build key from:

- compiler version and Pop Lang edition;
- exact command profile, platform target, and selected Bubble kinds/names;
- canonical `bubble.lock` bytes and selected feature sets;
- normalized manifest and source bytes for every selected Package;
- exact public dependency identities/API hashes and native provider facts; and
- the PLRI ABI and backend capability contract.

Absolute checkout paths, file metadata, timestamps, locale, terminal settings,
and human output do not enter the key.

The unstable internal cache lives below the selected shared `target/` root. A
cache record inventories every reusable output by normalized relative path,
size, and SHA-256 digest. Reuse requires an exact key, canonical record, regular
non-symlink output files, and matching bytes. A missing, malformed, stale, or
hash-mismatched record is a miss, never a partial hit. Publication stages and
verifies the complete record after successful object emission/linking. Cache
records contain no absolute paths or credentials.

## Consequences

- Features remain statically resolved manifest capabilities and never become
  runtime flags or conditional parsing.
- Platform dependencies and artifacts are selected by an explicit exact input,
  with no silent host fallback after a target is supplied.
- A second unchanged build can reuse verified outputs without trusting mtimes.
- Changing a source, manifest, feature, target, dependency identity, compiler,
  ABI, or native-provider fact invalidates reuse deterministically.
- Richer feature forwarding, default sets, resources, gated targets, and
  conditional source require a later accepted ADR.

## Alternatives considered

### Import Cargo feature syntax wholesale

Rejected because implicit defaults, feature forwarding, and conditional target
behavior would answer unaccepted Pop Lang design questions.

### Use modification times as incremental authority

Rejected because timestamps and checkout location are not semantic inputs and
cannot prove artifact bytes.

### Accept a target option but continue using the host internally

Rejected because that would mis-select dependencies, FFI layouts, link plans,
lock identities, and artifacts.

## Required conformance tests

- feature declaration parsing rejects unknown members, noncanonical names,
  duplicates, dangling optional dependencies, and non-optional dependency
  activation;
- feature selection is order-independent, enters the lock and dependency graph,
  and an optional dependency is unavailable without its enabling feature;
- exact platform dependencies and native plans follow the explicit target, and
  unsupported/unknown targets fail without host fallback;
- an unchanged second build reuses verified outputs; source, manifest, feature,
  target, lock, compiler/ABI, or provider changes miss the cache;
- corrupt records, symlinked outputs, size/hash mismatches, and path traversal
  are misses or errors without executing or linking stale bytes; and
- cache records and keys are stable across equivalent checkout roots.

## Documents/components affected

Package manifest parsing, lock resolution, unified CLI controls, native build
selection, artifact emission, Workspace target layout, architecture
conformance, closed decisions, and the implementation roadmap.
