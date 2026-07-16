# ADR 0055: Deterministic Lock and Poplib Encoding

- Status: accepted
- Date: 2026-07-14
- Depends on: ADR 0007, ADR 0014, ADR 0017, ADR 0028, ADR 0036, ADR 0054
- Supersedes: the open physical-encoding portions of those decisions
- Amended by: ADR 0096 for optional canonical typed
  `retained-adapters.popc` and its public adapter-only JSON references

## Context

The accepted Package/Bubble architecture requires one deterministic
`bubble.lock` and self-describing `.poplib` artifacts, but it defines only their
logical contents. Leaving the physical representation unspecified prevents
round-trip, malformed-input, reproducibility, hash, loader, and compatibility
tests. It also risks creating a second semantic path beside the already verified
logical reference-metadata model.

The first release needs an inspectable encoding with a small trusted parser. It
must carry generic specialization capsules without exposing dependency source,
widening visibility, or turning compiler debug dumps into stable inputs.

## Decision

### Common encoding and integrity rules

`bubble.lock`, `bubble.manifest`, and `reference.metadata` use schema-versioned
canonical UTF-8 JSON. Canonical files use the schema's fixed object-member
order, identity-sorted arrays, deterministic decimal integers, minimal JSON
string escaping, no insignificant whitespace, and exactly one trailing newline.
Object keys are unique. Unknown fields, duplicate fields, unsupported schema
versions, invalid UTF-8, noncanonical encodings, excessive nesting or counts,
and files above their declared budgets fail closed.

All content digests are lowercase hexadecimal SHA-256. File sizes are recorded
and checked before hashing or parsing. Artifact inventories use normalized
forward-slash relative paths, sort by path, reject `.`/`..`, absolute paths,
backslashes, duplicate paths, symlinks, devices, and entries outside the
artifact root. The manifest is outside its own file inventory to avoid recursive
hashing; the artifact digest hashes the canonical manifest bytes followed by the
ordered `(path, size, digest)` inventory projection.

The Rust bootstrap implementation may use the reviewed `serde`, `serde_json`,
and `sha2` crates inside project/artifact/compiler-driver ownership. The
backend-neutral foundation, resolver, type, and HIR crates may depend only on
`serde` to derive the closed data-model projection consumed by the artifact
owner; they do not select a physical encoding, parse files, hash content, or
perform I/O. These are host implementation dependencies, do not enter Pop
source or HIR/MIR semantics, and do not authorize ambient runtime JSON parsing
or cryptographic APIs.

### `bubble.lock` schema version 1

One Workspace-root `bubble.lock` records:

- `schemaVersion`, resolver version, selected platform target, and generation
  inputs that affect resolution;
- exact Package name, version, source kind, source identity, content SHA-256,
  and sorted additive features;
- each selected Bubble name/kind and its exact direct Bubble dependencies;
- selected artifact location, artifact digest, public API digest, PLRI ABI
  range, edition, and required capabilities when an artifact is available.

Source kinds are registry, exact Git revision, and normalized local path. Local
paths are Workspace-relative resolution inputs and never semantic identity.
Credentials, ambient absolute checkout paths, timestamps, and human diagnostics
are forbidden. Package and Bubble records are identity-sorted; dependency and
feature arrays are sorted and duplicate-free. The compile-time Bubble graph is
acyclic. Runtime initialization edges are retained and validated separately.

`--locked` rejects any byte change to an existing canonical lock. `--offline`
forbids registry/Git transport but may use already verified cache entries and
local paths. `--frozen` applies both rules. A normal resolving command writes a
new lock atomically only after complete resolution and verification.

### `.poplib` schema version 1

The logical directory remains:

```text
<BubbleName>.poplib/
  bubble.manifest
  reference.metadata
  retained-adapters.popc
  documentation.xml
  targets/
    <platform-target>/native.object
  resources/
```

`bubble.manifest` records the complete `BubbleIdentity`, Package identity and
source digest, Bubble kind, language edition, manifest/reference/capsule schema
versions, compiler compatibility, PLRI ABI range, required capabilities, exact
direct Bubble identities, sorted public namespace index, initialization order,
reference-only status, target implementations, optional documentation, and the
ordered file inventory. Documentation has its own digest and never changes the
public API or implementation digest.

When public ADR 0096 schemas exist, the artifact additionally inventories a
public-only canonical typed `retained-adapters.popc` with its independent
schema/protocol versions, size, and full SHA-256. This optional file is not a
JSON control file.

`reference.metadata` is a serialization of the verified logical
`ReferenceMetadata` model, not a parallel resolver contract. It contains only
public names, signatures, layouts/contracts, constants/UDA projections, exact
referenced Bubble identities, and portable generic entries. Internal/private
names never enter consumer lookup.

ADR 0096 public generated schema entries add only their stable
`Codec.Schema<T>` identity, exact type, and `.popc` path/size/full file and entry
fingerprints to this JSON file. The record/enum/union projection and provenance
remain exclusively in `.popc`; JSON cannot duplicate or replace that schema.

A public generic entry records its capsule schema version, source
`SymbolIdentity`, canonical content SHA-256, resource counts, and its verified
opaque HIR/type payload. Schema version 1 may encode that payload inline in the
canonical reference file. A later schema may move payload bytes to separately
hashed files without changing generic semantics. Loading verifies the file and
capsule hashes, owner, dependency closure, type graph, effects, HIR invariants,
visibility closure, and budgets before specialization. It never parses source,
merges dependency Modules, or resolves runtime names.

Target implementation bytes remain backend-owned opaque files. Native objects
are selected only after manifest, target, capability, PLRI ABI, file size, and
SHA-256 verification. Reference-only loading maps no executable content.

Emission uses a sibling temporary directory, closes and hashes every file,
writes the manifest last, verifies the complete temporary artifact through the
normal loader, and atomically publishes it. Loading never probes ambient working
directories; the verified lock graph supplies every artifact path.

### Compatibility

Schema versions are independent for locks, manifests, reference metadata,
retained-adapter `.popc`, generic capsules, documentation, and target
implementations. Version 1 readers
reject unsupported versions and unknown critical fields. The `0.1.0` release
freezes the version-1 byte fixtures and compatibility ranges; before that freeze,
repository caches may be invalidated but accepted conformance fixtures remain
deterministic.

## Consequences

- Lock and artifact bytes can be reproduced and compared across checkout paths
  and input enumeration order.
- Public metadata still flows from verified HIR; serialization cannot add
  dynamic types, source lookup, or a backend-specific semantic path.
- SHA-256 file inventories support corruption and supply-chain verification;
  signatures remain optional manifest data until publishing policy supplies
  trusted keys.
- Canonical JSON is larger than a custom binary format, but it is inspectable,
  versioned, bounded, and adequate for `0.1.0`. Packed directory transport can
  be added without changing the logical files.
- The loader and resolver must expose typed closed errors rather than parsing
  human messages.

## Alternatives considered

### Treat Rust debug or HIR/MIR dump text as the format

Rejected because those are disposable compiler-version debug views, lack a
closed parser contract, and would accidentally stabilize implementation detail.

### Store dependency source and recompile it in consumers

Rejected because it collapses Bubble identity, visibility, initialization, and
artifact independence.

### Use an unversioned ad-hoc binary encoding

Rejected because malformed-input handling, compatibility, inspection, and
reproducibility would be underspecified.

### Use noncryptographic compiler hashes for file integrity

Rejected because cache-key repeatability is not sufficient for downloaded or
published artifact integrity.

### Put documentation inside reference metadata

Rejected because documentation-only edits must not change API/ABI identity and
runtime/reference loading does not require documentation.

## Required conformance tests

- canonical lock/manifest/reference fixtures round-trip byte-for-byte and are
  invariant under input ordering, absolute checkout path, and timestamp;
- duplicate/unknown fields, noncanonical JSON, path traversal, symlinks,
  oversized inputs, unsupported versions, and malformed hashes fail closed;
- SHA-256 inventory, API, implementation, capsule, documentation, and complete
  artifact digests detect single-byte changes in their owned domains;
- lock generation covers registry, exact-Git, local-path, features, platform
  selection, transitive graphs, cycles, and `--locked`/`--offline`/`--frozen`;
- reference metadata round-trips every accepted recursive type and public
  generic capsule while excluding `internal`/`private` names;
- a consumer loads and executes ordinary and generic APIs from `.poplib`
  without dependency source and with exact `SymbolIdentity` retention;
- target, capability, edition, dependency, PLRI ABI, implementation hash, and
  initialization mismatches fail before executable mapping;
- documentation-only changes alter only the documentation and complete artifact
  digests, not public API or implementation digests;
- interpreter/LLVM tests consume the same verified logical metadata after disk
  round-trip; HIR/MIR remain backend-neutral.

## Documents/components affected

Project resolver, lock writer/reader, Bubble artifact emitter/loader, reference
metadata, portable generic capsules, compiler driver, linker, documentation
generator, cache keys, Package/Workspace CLI, architecture tests, and release
fixtures.
