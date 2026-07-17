# ADR 0091: Canonical Package Scaffolding

- Status: accepted
- Date: 2026-07-16
- Supersedes: none
- Amends: ADR 0017, ADR 0088

## Context

The CLI architecture reserves `pop new` and `pop initialize`, and the Package
architecture fixes `bubble.toml`, `src/lib.pop`, and `src/main.pop`, but the
bootstrap driver implements neither command. Scaffolding without a precise
contract risks inventing a second layout, deriving invalid identities from
paths, overwriting work, or generating source that the compiler itself rejects.

## Decision

`pop new <path>` creates a new Package directory. `pop initialize [path]`
initializes an existing directory and defaults to the current directory. Both
commands accept:

```text
--name <Package.Name>
--library | --binary
```

The default kind is `binary`. When `--name` is absent, the final directory
component is used only if it is already a valid PascalCase Package identity;
there is no dash, underscore, case, or punctuation rewriting. Invalid or
ambiguous derivation requires `--name`.

The initial scaffold contains exactly `bubble.toml` and either `src/main.pop`
for a binary or `src/lib.pop` for a library. The manifest declares the selected
name, version `0.1.0`, and edition `2026`. The root source declares the Package
namespace. A binary adds the exact private no-result `function main()`
shorthand. A library begins with no invented public API. Generated text uses
deterministic LF line endings and passes the same manifest, parser, and
front-end validation as handwritten inputs.

`pop new` requires the destination not to exist. `pop initialize` requires an
existing directory with no `bubble.toml`, `src/lib.pop`, or `src/main.pop` and
never overwrites any entry. Creation occurs in a sibling temporary directory;
validated files are renamed into place only after the complete scaffold is
ready. On failure the command removes only its own temporary output. It never
initializes version control, downloads dependencies, writes credentials, or
selects a registry.

Successful human output reports the Package identity and destination through
the selected immutable locale. Command names, paths, identities, generated
source, and manifest values are not translated.

Workspace-only scaffolding, combined Package/Workspace roots, custom editions,
templates, licenses, tests, CI, README content, and VCS initialization require
later explicit options or decisions.

## Consequences

- A new developer can create a buildable canonical Pop Lang Package with one
  command.
- Generated Packages remain minimal and teach only accepted syntax and layout.
- Existing directories and identities are never silently rewritten.

## Alternatives considered

### Copy a large application template

Rejected because optional documentation, tests, dependencies, and policies do
not belong in every Package.

### Normalize arbitrary directory names

Rejected because paths are resolution inputs, not authority to choose a
Package identity.

### Write files directly into the destination

Rejected because a partial scaffold after failure is difficult to distinguish
from user work.

## Required conformance tests

- default binary and explicit library scaffolds have exact deterministic files;
- generated manifests parse and generated sources pass compiler analysis;
- `initialize` defaults to the current directory;
- invalid derived and explicit names are rejected;
- conflicting kind options, extra arguments, existing destinations, protected
  entries, and symlink destinations are rejected without modifying user files;
- a simulated validation or rename failure leaves no partial destination; and
- all human messages exist in every official locale with placeholder parity.

## Documents/components affected

CLI/tooling architecture, Package layout, implementation roadmap, driver
argument parsing and scaffolding service, localization catalogs, CLI tests, and
architecture conformance tests.
