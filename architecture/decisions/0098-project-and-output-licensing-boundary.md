# ADR 0098: Project and Output Licensing Boundary

- Status: accepted
- Date: 2026-07-21
- Supersedes: the repository-wide permissive license declaration
- Amended: 2026-07-21 to use family-local `LICENSE.txt` files

## Context

Pop Lang previously used one license declaration for the complete repository.
That does not express the intended distinction between the toolchain itself and
the components that are linked, embedded, copied, or otherwise distributed
with programs built by the toolchain.

The compiler and developer tools should use a strong copyleft license. Runtime
services, foundational libraries, official extension Packages, examples, and
project-owned generated glue must remain usable in applications under a
permissive license. The boundary must be explicit enough for Cargo metadata,
source archives, generated artifacts, package publication, and automated
conformance checks to agree.

Third-party components retain their own licenses and notices.

## Decision

Copyright notices for the Pop Lang project name Julia Klee as:

```text
Copyright (C) 2026 Julia Klee
```

The repository uses the exact SPDX identifiers `GPL-3.0-only` and
`Apache-2.0`. The root `LICENSE.txt` is the human-readable boundary index and
references the complete license text at each Cargo family boundary:

- `crates/compiler/LICENSE.txt` and `crates/tools/LICENSE.txt` contain GNU GPL
  version 3;
- `crates/extensions/LICENSE.txt`, `crates/libraries/LICENSE.txt`, and
  `crates/runtime/LICENSE.txt` contain Apache License 2.0.

The repository does not keep an obsolete project license file or describe the
superseded repository-wide terms as a current Pop Lang license.

The default license is `GPL-3.0-only`. It applies when no narrower path or
file-level declaration says otherwise, including:

- compiler front ends, HIR, MIR, backends, and compiler drivers under
  `crates/compiler/`;
- developer tools under `crates/tools/`;
- architecture, specifications, project documentation, localization assets,
  tests owned by those components, and repository administration files.

The following application-facing trees are `Apache-2.0`:

- `crates/runtime/`;
- `crates/libraries/` and `libraries/`;
- `crates/extensions/`;
- `examples/`.

This Apache boundary follows the semantic ownership of runtime services,
`Pop.Internal`, `Pop.Standard`, and official extension Packages. It applies to
their source, manifests, focused tests, documentation inside those trees, and
their independently distributed binary or `.poplib` forms.

Using the compiler, formatter, language server, documentation generator, test
runner, or package tools does not place user-authored Pop source or an ordinary
output derived from that source under the GPL. The project claims no copyright
in user-authored input merely because a tool processed it.

To the extent a tool copies copyrightable Pop Lang project material into an
output, the copied project-owned material is available under `Apache-2.0`.
This includes generated binding declarations and closed C shims, package
scaffolding text, native link glue, runtime support, foundation-library code,
extension code, and equivalent future application-facing templates. A
generator implementation can remain GPL-3.0-only; that implementation license
does not replace the explicit Apache-2.0 license of project-owned material it
emits. User-provided names, declarations, descriptors, and other input remain
under terms selected by their owner.

An artifact or distribution containing both license classes retains a component
license inventory and both license texts. Apache-2.0 material may be combined
into a GPLv3 distribution, but doing so does not change the standalone source
license of the Apache components. GPL implementation code must not be copied
into an Apache-only runtime, library, extension, template, shim, or generated
output.

Cargo manifests use the license of the published crate rather than one
repository-wide inherited value:

- compiler and tool crates inherit workspace `GPL-3.0-only`;
- runtime, library, and extension crates declare `Apache-2.0` explicitly.

Future contributions are accepted under the license assigned to their target
path. Moving code across the boundary requires explicit review of authorship,
notices, dependencies, generated content, and distribution consequences.

## Consequences

- Changes to the compiler and tooling are distributed under GPL version 3 only.
- Applications can link or redistribute the Pop Lang runtime, foundation
  libraries, and official extensions under Apache License 2.0 terms without
  inheriting the toolchain GPL solely from those components.
- Cargo metadata describes each independently publishable crate accurately.
- Each requested crate family carries its complete governing license text next
  to its members, while the root index links the complete boundary.
- Generated-output policy is explicit and cannot be inferred from the license
  of the generator executable.
- Releases and package archives need a deterministic component license
  inventory in addition to dependency license reporting.

## Alternatives considered

### License the complete repository under GPL-3.0-only

Rejected because runtime, library, extension, and copied support code are part
of user applications and should not impose the toolchain copyleft boundary on
those applications.

### Keep the previous repository-wide license

Rejected because it does not provide the requested copyleft protection for the
compiler and developer tooling.

### Use one dual-license expression everywhere

Rejected because it would allow the compiler and tools to be selected under
Apache-2.0 and would obscure which components are deliberately safe to embed or
link into applications.

### Treat every compiler output as Apache-2.0

Rejected because the project does not own user-authored source or all material
derived from it. Only copyrightable Pop Lang project material copied into an
output receives the project Apache-2.0 grant.

## Required conformance tests

- root `LICENSE.txt` references every family-local license file and names the
  exact SPDX identifiers;
- compiler/tools contain the complete GNU GPL version 3 text, while runtime,
  library, and extension families contain the complete Apache License 2.0 text;
- no obsolete project license file or stale project-license reference remains;
- workspace metadata is `GPL-3.0-only`;
- every compiler/tool Cargo package inherits `GPL-3.0-only`;
- every runtime/library/extension Cargo package declares `Apache-2.0` and does
  not inherit the GPL workspace value;
- application-facing source trees cannot silently acquire a GPL file-level or
  package declaration;
- release, `.poplib`, and Package metadata retain exact component license and
  notice inventories;
- generator and scaffold regression tests prove that user output is not marked
  GPL and that copied project-owned support material stays inside the Apache
  boundary;
- architecture link and terminology checks include the licensing document and
  ADR.

## Documents/components affected

Root license and notice files, README and contributor policy, Cargo workspace
and member manifests, runtime and base-library architecture, extension Package
architecture, artifact/package publishing, generated bindings and scaffolding,
release/SBOM metadata, architecture conformance tests, and closed decisions.
