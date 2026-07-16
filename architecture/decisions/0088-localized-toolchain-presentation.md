# ADR 0088: Localized Toolchain Presentation

- Status: accepted
- Date: 2026-07-16
- Supersedes: none
- Amends: ADR 0010, ADR 0017, ADR 0018

## Context

ADR 0010 requires compiler passes to emit structured diagnostics and requires
human, JSON, LSP, SARIF, and test consumers to share one diagnostic model. The
implementation already carries stable codes, message keys, and typed arguments,
but the bootstrap CLI still renders many English strings directly and diagnostic
output exposes only codes and spans. The language server has no session locale.

Toolchain presentation is separate from application localization. The planned
public `Pop.Locale` and `Pop.Resource` APIs compile typed application resources
from YAML. The compiler, CLI, language server, formatter, documentation tool,
test runner, and installer cannot depend on those public libraries without
creating a bootstrap and layering cycle.

## Decision

Pop Lang toolchain presentation uses one private, statically keyed localization
subsystem shared by every official tool. It is not a public standard-library API.

### Catalogs and supported languages

Toolchain catalogs are UTF-8 TOML fragments below
`assets/locales/<tag>/<component>/` and are embedded in each distributed tool.
English (`en`) is the canonical schema. Components own focused fragments such
as `cli/help.toml`, `compiler/lexer.toml`, `compiler/parser.toml`,
`compiler/types.toml`, and `lsp/session.toml`; no language uses one monolithic
catalog file. A locale `manifest.toml` declares its canonical tag. The first
complete catalog set is:

- English: `en`;
- Simplified Chinese: `zh-Hans`;
- Japanese: `ja`;
- Brazilian Portuguese: `pt-BR`;
- neutral Spanish: `es`.

The aggregate of every official locale contains exactly the canonical keys and
exactly the same named placeholders. Missing fragments, missing keys, extra
keys, duplicate keys across fragments, malformed TOML, unknown placeholders,
and placeholder mismatches are build/test failures. An official release cannot
silently fall back for a missing translated key.

Message keys are private toolchain identities and may evolve with the toolchain.
Stable diagnostic identities remain the `POP####` codes. Translated text is not
a machine protocol or compatibility identity.

### Selection and fallback

One immutable `Language` and `RenderContext` are selected for each CLI invocation
or language-server session. Selection precedence is:

1. the global `--language <tag>` CLI option or LSP initialization locale;
2. `POP_LANGUAGE`;
3. the `language` field in the user tool configuration;
4. `LC_ALL`, then `LC_MESSAGES`, then `LANG`;
5. English.

The CLI pre-scans the global option before normal argument parsing so usage
errors can use the requested language. Locale normalization accepts `_` as a
separator and ignores POSIX encoding/modifier suffixes. Fallback is exact tag,
then language and script, then language, then a documented alias, then English.
The initial aliases are `zh`, `zh-CN`, and `zh-SG` to `zh-Hans`; `pt` and
`pt-BR` to `pt-BR`; `ja-*` to `ja`; and `es-*` to `es`. Unsupported explicit
tags are usage errors rather than silent selection changes.

User configuration is read without executing code from
`$XDG_CONFIG_HOME/pop/config.toml`, or `$HOME/.config/pop/config.toml` when
`XDG_CONFIG_HOME` is absent. A malformed explicit configuration is reported;
unavailable ambient configuration does not prevent compiler operation.

### Rendering boundary

Compiler passes, loaders, and semantic queries produce typed facts, error kinds,
diagnostics, or tool events. They do not select a locale or assemble final human
sentences. CLI, LSP, and other presentation adapters render those facts with the
session `RenderContext`.

Named placeholders are required. Values are escaped and substituted as data;
they never select message keys or dispatch behavior. Diagnostic argument kinds
remain statically closed. Catalog text cannot emit terminal control characters.
The English catalog remains available for incidents that occur before a valid
context exists.

Human diagnostic headings, messages, labels, notes, and fix titles are localized.
Diagnostic codes, source text, identifiers, type names, paths, target triples,
package identities, and user-provided values are preserved exactly.

### Machine and external text

JSON, SARIF, LSP protocol fields, enum values, codes, symbol IDs, build-event
identities, HIR/MIR/LLVM dumps, and process exit codes are locale invariant. A
machine format may include an explicitly labeled localized display field, but
consumers never need it to recover semantic facts. LSP selects a locale from the
standard initialization locale while continuing to transport stable codes and
typed data.

User program output is never translated by the toolchain. Operating-system,
linker, and third-party text may be preserved verbatim as a nested external
detail; the Pop-owned context around it is localized.

### Completeness

“Complete localization” means every human presentation message currently owned
by the shipped Pop toolchain has a catalog key in all five official catalogs.
It does not mean translating source code, identifiers, artifacts, machine
schemas, debug IR, user output, or external operating-system text. Human review
is required in addition to mechanical catalog parity.

## Consequences

- Toolchain binaries grow by the size of five embedded text catalogs.
- A small private localization crate becomes a dependency of presentation tools,
  but never of compiler semantic layers or `Pop.Standard`.
- CLI errors must migrate from ad hoc strings toward typed presentation errors.
- Locale selection is deterministic and testable without mutable global state.
- Application localization keeps its independent YAML and generated-key design.

## Alternatives considered

### Use YAML for toolchain catalogs

Rejected. YAML remains the authoring format for typed application resources,
while the closed toolchain catalog benefits from TOML's smaller parser surface
and existing configuration conventions. Sharing a format would not justify a
dependency from bootstrap tools to public resource packages.

### Use one ambient process locale

Rejected because language-server clients can use different locales, tests need
deterministic contexts, and mutable global locale makes concurrent rendering
unsafe.

### Translate final English strings

Rejected because runtime string lookup is unstable, prevents placeholder
validation, and violates the structured diagnostic contract.

### Localize machine schemas and identifiers

Rejected because it would make automation locale-dependent and destroy stable
protocol identities.

## Required conformance tests

- exact key and named-placeholder parity for all five embedded catalogs;
- deterministic normalization, alias, precedence, and unsupported-tag tests;
- explicit-language CLI help and usage errors in all five languages;
- compiler diagnostic message, label, note, and fix rendering in all languages;
- language-server sessions with independent locales;
- machine-code, argument, path, and source preservation across locales;
- malformed configuration and catalog rejection;
- architecture regression checks preventing new direct human presentation text
  in compiler passes and preventing dependencies from semantic crates or the
  base libraries to the localization crate.

## Documents/components affected

Diagnostics, CLI/tooling architecture, closed decisions, architecture
conformance tests, compiler driver presentation, language server, formatter,
documentation generator, test runner, installer integration, release packaging,
and `assets/locales/`.
