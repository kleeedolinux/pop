# ADR 0107: Bootstrap Auxiliary Bubble Execution

- Status: accepted
- Date: 2026-07-26
- Supersedes: none
- Amends: architecture 14, architecture 21

## Context

Accepted Package layout already makes each `tests/*.pop`, `examples/*.pop`, and
`benchmarks/*.pop` root a distinct Bubble that depends only on the Package
library's public API and declared development dependencies. It also requires
those Bubbles to emit executables, but it does not define a runnable bootstrap
contract before the planned `Pop.Test` and `Pop.Benchmark` libraries and their
generated harness metadata exist.

Treating discovery alone as completed execution would leave `pop test`
nonfunctional. Scanning arbitrary functions or attributes at runtime would
introduce reflection and string-dispatch behavior forbidden by the language
architecture.

## Decision

Until a later accepted ADR introduces compiler-generated typed test or benchmark
harness metadata, every conventional test, example, and benchmark Bubble is an
explicit executable Bubble. Its root Module must contain exactly one entry item
with the same accepted static signatures and private visibility rules as a
binary entry:

```luau
function main()
end
```

or:

```luau
private function main(arguments: Array<String>): Int
    return 0
end
```

This is a bootstrap harness boundary, not a claim that future `@Test` items are
ordinary entry functions.

`pop check` analyzes every discovered Bubble. Normal dependencies remain
available to library and binary Bubbles; development dependencies are added
only to test, example, and benchmark Bubbles. Their availability never widens
Package-library visibility or enters its public metadata.

`pop test --manifestPath <bubble.toml>` builds and executes every selected test
Bubble in deterministic Package-name/Bubble-name order. It executes every test
Bubble even after a failure and succeeds only when all exit successfully.
Termination by signal or an exit status outside the portable unsigned-byte
range is failure. Structured command output includes one versioned test result
event per Bubble and the normal final command event.

Example and benchmark Bubbles use the same deterministic executable lowering
and native-link path. `pop build` may select them explicitly through the
architecture-defined Bubble selectors; an ordinary unselected build continues
to build only the Package library and binary Bubbles. A later accepted test or
benchmark harness must replace this entry contract and synchronize the CLI,
metadata, documentation, and conformance tests.

## Consequences

- Test, example, and benchmark roots are compiled as semantically distinct
  Bubbles rather than folded into a library or binary Bubble.
- The bootstrap runner requires no runtime reflection, name scanning, implicit
  globals, or dynamic calls.
- Development dependencies have a test/example/benchmark-only proof boundary.
- Multiple test executables produce deterministic complete results rather than
  fail-fast order dependence.
- Planned `@Test`, property, fixture, and benchmark APIs remain unimplemented
  until their typed generated-harness contract is accepted.

## Alternatives considered

### Discover `@Test` functions by name or retained metadata

Rejected because the typed generated harness and `Pop.Test` failure protocol
are not accepted or implemented, and runtime enumeration would violate the
reflection boundary.

### Treat successful compilation as a passing test

Rejected because `pop test` must execute behavior and report its process
outcome.

### Fold auxiliary roots into the Package binary

Rejected because accepted architecture makes each root a separate Bubble with
its own identity, visibility, dependencies, artifact, and execution result.

## Required conformance tests

- `pop check` rejects an invalid test, example, or benchmark Bubble that an
  ordinary library/binary build would otherwise not read;
- a test Bubble can use the Package library public API and an exact
  development dependency, while a library or binary Bubble cannot use that
  dependency;
- multiple test Bubbles execute in deterministic identity order, all results
  are reported, and any nonzero result fails the command;
- test, example, and benchmark executables link through the same exact public
  `SymbolIdentity`, native provider, target, and capability validation as
  binary Bubbles; and
- no runtime name scan, attribute scan, reflection lookup, or dynamic call is
  introduced by the bootstrap runner.

## Documents/components affected

Package/Bubble discovery, unified CLI selection, dependency scopes, native
linking, build events, architecture conformance, closed decisions, and the
implementation roadmap.
