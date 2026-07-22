# Project and Output Licensing

## License classes

Pop Lang uses a deliberate source and distribution boundary defined by
[ADR 0098](./decisions/0098-project-and-output-licensing-boundary.md):

| Class | SPDX identifier | Ownership |
| --- | --- | --- |
| Toolchain | `GPL-3.0-only` | compiler, backends, developer tools, architecture, and repository material not assigned more narrowly |
| Application-facing | `Apache-2.0` | runtime, foundational libraries, official extensions, examples, and project-owned material copied or linked into user artifacts |

The copyright notice is `Copyright (C) 2026 Julia Klee`. Contributor and
third-party notices remain intact.

The root `LICENSE.txt` is the repository path map. Full license texts live at
the crate-family boundary that they govern:

```text
crates/compiler/LICENSE.txt    GPL-3.0-only
crates/tools/LICENSE.txt       GPL-3.0-only
crates/extensions/LICENSE.txt  Apache-2.0
crates/libraries/LICENSE.txt   Apache-2.0
crates/runtime/LICENSE.txt     Apache-2.0
```

There is no obsolete Pop Lang project license file. Root material that is not
assigned more narrowly remains `GPL-3.0-only` and uses the GNU text referenced
by the root index.

## Source boundary

The default for a file without a narrower declaration is `GPL-3.0-only`.
Compiler and tool Cargo crates inherit that workspace value.

These trees are application-facing and use `Apache-2.0`:

```text
crates/runtime/
crates/libraries/
libraries/
crates/extensions/
examples/
```

Their tests and local documentation follow the owning component. Moving a file
across this boundary is a licensing change, not a mechanical refactor. Review
must preserve copyright and third-party notices and must prevent GPL
implementation code from entering an Apache-only application component.

## Programs and generated output

Running a Pop Lang tool does not license user-authored source or ordinary output
to the project and does not make it GPL-covered merely because the tool is GPL.
Users select the license for their own program.

Project-owned runtime, library, extension, example, template, scaffold, binding,
shim, link-glue, or other support material copied or linked into an application
is available under `Apache-2.0`. Only that project-owned material receives this
grant; user input and third-party content retain their own terms. A GPL
generator implementation remains separate from the Apache status of the
project-owned bytes it is designed to emit.

## Artifacts and publishing

Every distributable archive, `.poplib`, toolchain bundle, generated support
bundle, and release manifest records the licenses of the components it actually
contains. Mixed distributions include both license texts and all applicable
notices. Source-only compiler output containing no Pop Lang project code needs
no project license notice.

`pop package` and `pop publish` preserve declared Package license metadata.
Foundation and official extension Packages declare `Apache-2.0`; a normal user
Package keeps the license selected by its author. Scaffolding does not silently
choose a license for a new user Package.

Dependency license reports remain distinct from the Pop Lang component license
inventory. External dependencies keep their own terms and notices.

## Conformance

Architecture checks validate the root index, complete family-local license
texts, absence of an obsolete project license, workspace default, and every
member Cargo manifest. Release and package conformance must also prove that:

- GPL implementation source is absent from Apache-only runtime/library/
  extension and generated-support payloads;
- application-facing artifacts declare `Apache-2.0` exactly;
- a compiler or tool binary retains `GPL-3.0-only` metadata;
- mixed bundles carry both license texts and applicable notices;
- generated user code is never labeled GPL solely because the generator is
  GPL-covered.
