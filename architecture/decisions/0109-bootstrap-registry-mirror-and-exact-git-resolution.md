# ADR 0109: Bootstrap Registry Mirror and Exact-Git Resolution

- Status: accepted
- Date: 2026-07-26
- Supersedes: none
- Amends: ADR 0017, ADR 0055, architecture 21

## Context

Accepted Package architecture names registry, exact-Git revision, and local path
as initial dependency sources. The bootstrap parser preserves all three, but the
driver currently resolves only local paths and reports that registry/Git
resolution is absent. No online Pop Lang registry publication, authentication,
or archive transport protocol is accepted yet, so inventing one inside the
compiler driver would exceed architecture.

The first release still needs deterministic conformance for registry identity,
exact Git revision, source hashing, lock policy, offline reuse, and cache
tamper detection.

## Decision

### Explicit bootstrap registry mirror

Manifest-driven resolver commands accept
`--registryRoot <directory>` as an explicit bootstrap capability. The directory
is a pre-provisioned local mirror with this closed layout:

```text
<registryRoot>/<DependencyAlias>/<exact-version>/bubble.toml
```

The selected directory must be real, non-symlinked, and remain below the
canonical registry root. The contained Package manifest supplies and validates
the semantic Package/Bubble identity; the dependency alias remains lookup
syntax only. The resolver verifies the declared exact version/Bubble, hashes
the normalized manifest and selected source bytes, and records
`{"kind":"registry","identity":"default"}` plus that content hash in
`bubble.lock`. The absolute mirror path never enters the lock, cache key,
artifact, or diagnostic schema.

This mirror is a bootstrap resolution input, not the future online registry,
publisher, credential, index, signing, or archive protocol. Missing
`--registryRoot` for a selected registry dependency is a clear capability
error. Supplying it authorizes read-only local resolution only.

### Exact-Git cache

An exact-Git dependency resolves into:

```text
<workspace-or-package>/target/resolution/exactGit/<source-key>/checkout/
```

The source key hashes the repository locator and exact revision. Normal mode may
invoke the host `git` executable without a shell to fetch that revision into a
new staging directory. Publication occurs only after:

- `git rev-parse HEAD` exactly equals the requested full revision;
- a regular non-symlink `bubble.toml` parses and matches the declared
  version/Bubble;
- normalized selected source bytes hash successfully; and
- a canonical source record stores repository, revision, and content SHA-256.

Reuse re-hashes the checkout and requires an exact canonical source record.
Mutation, symlink substitution, identity mismatch, or malformed metadata fails
closed; it never silently updates the locked source. Credentials embedded in a
repository locator are rejected.

`--offline` and `--frozen` never invoke Git or any registry/network transport.
They may reuse only a previously verified exact-Git checkout and may read the
explicit local registry mirror. `--locked` additionally requires the resulting
canonical graph bytes to match the existing lock.

### Resolution graph

Local, registry-mirror, and exact-Git Packages enter the same deterministic
cycle check, public reference-metadata lowering, Bubble edge validation, native
link-plan merge, target/feature selection, and build-cache key. Two routes to
one canonical Package identity must agree on source identity and content hash;
otherwise resolution fails rather than choosing by traversal order.

## Consequences

- All architecture-defined source kinds have deterministic bootstrap resolution
  and lock identities.
- Online registry distribution remains unclaimed until its protocol, signing,
  credentials, archive, and dependency reviews are accepted.
- Git is an explicit resolver process capability and is never invoked through a
  shell or during offline/frozen work.
- Registry/Git source paths and credentials do not leak into lock or cache
  identities.

## Alternatives considered

### Invent an HTTP registry endpoint in the compiler driver

Rejected because registry indexing, signing, authentication, archive encoding,
and publication are separate open distribution decisions.

### Treat a Git working tree as an ordinary local path

Rejected because that loses the exact revision source identity and permits
mutable branch state.

### Trust an existing checkout without a source record

Rejected because directory names and mtimes do not prove revision or content.

## Required conformance tests

- a registry mirror dependency resolves, compiles, records registry identity
  and content hash, and remains checkout-location-independent;
- missing, escaping, symlinked, version-mismatched, or Bubble-mismatched mirror
  entries fail closed;
- an exact local Git revision fetches once, records exact-Git identity, reuses
  offline, and rejects a different or mutated checkout;
- offline/frozen never invoke Git when the verified cache is absent;
- dependency cycles and conflicting identities are rejected across mixed local,
  registry, and Git sources; and
- repository credentials and absolute resolver/cache paths never enter lock or
  build-cache bytes.

## Documents/components affected

Package resolution, unified CLI controls, lock generation/policy, source cache,
Git process capability, Workspace target layout, architecture conformance,
closed decisions, and the implementation roadmap.
