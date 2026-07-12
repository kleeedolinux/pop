# ADR 0028: Toolchain Distribution and `popup` Management

- Status: proposed
- Date: 2026-07-11
- Amends on acceptance: ADR 0017, ADR 0018

## Context

ADR 0017 establishes `pop` as the unified command for compiling, testing,
documenting, resolving, packaging, publishing, and installing Pop Lang Package
binary Bubbles. It does not define how a user obtains `pop` itself, installs
multiple compiler/runtime versions, selects a version for a Workspace, or
updates the toolchain manager. Its `pop install` command installs a public
binary Bubble from a Package; treating that command as a toolchain installer
would collapse two different artifact and trust domains.

ADR 0018 selects Rust 2024 and a checked Cargo workspace for the implementation.
It authorizes only reviewed external dependencies and currently defines no
toolchain-distribution, bootstrap, terminal-UI, network, archive, or signature
verification boundary.

The repository also lacks an accepted contract for a relocatable toolchain
archive, a release index, release channels, signing roots, update recovery,
version-selection precedence, or a one-script bootstrap. Implementing any of
those choices first would turn an architecture gap into a public security and
compatibility promise.

This proposal defines a separate toolchain manager named `popup`. It is modeled
after the useful separation between a language/package command and a toolchain
installer, without importing Rust source syntax, Package terminology, or
unrestricted build-script behavior into Pop Lang.

Because this ADR remains proposed, it does not yet authorize implementation,
new Cargo dependencies, release publication, or changes to the integrated
architecture. Acceptance requires the synchronized documents and failing
conformance tests listed below.

## Decision

If this ADR is accepted, Pop Lang will distribute immutable, verified,
relocatable toolchains through the `popup` toolchain manager and one narrow Bash
bootstrap script.

### Command ownership

`pop` and `popup` have non-overlapping responsibilities:

- `pop` remains the unified language, compiler, Workspace, Package, Bubble,
  documentation, diagnostic, and package-manager command;
- `pop install` continues to build and install a selected public binary Bubble
  from a Package;
- `popup` installs, verifies, selects, updates, and removes complete Pop Lang
  toolchain distributions;
- `popup` never resolves `bubble.toml` dependencies, edits `bubble.lock`,
  publishes Packages, or changes Item/Module/Bubble/Package/Workspace
  visibility;
- `pop` never silently changes or self-updates the selected toolchain.

The second executable is a deliberate, narrow amendment to ADR 0017's
single-user-facing-command rule. It exists because a command cannot reliably
install or replace itself before a usable `pop` toolchain is present.

The initial noninteractive command surface is:

```text
popup list [--installed|--available] [--includePrerelease]
popup install <version|stable>
popup default <version|stable>
popup run --toolchain <version|stable> -- <command> [arguments...]
popup update [<version|stable>]
popup uninstall <version>
popup doctor
popup self update
```

An exact version is preferred in automation and checked-in selection. `stable`
is the only initial moving channel. `latest` may appear in human presentation as
the release selected by `stable`, but is not a second independently resolved
channel. Preview releases are exact versions and are omitted unless explicitly
requested. Nightly, branch, commit, and source-build toolchains require a later
accepted decision.

### Toolchain distribution identity

A toolchain distribution is not a Package or `.poplib`. It is one immutable,
host-targeted release bundle containing a mutually compatible set of:

- the `pop` executable and first-party tools shipped for that release;
- the native runtime and the versioned PLRI implementation;
- the trusted `Pop.Internal` Bubble and public `Pop.Standard` Bubble;
- target adapters, linker/runtime support files, and other required shipped
  assets;
- license notices, release metadata, and a complete file inventory;
- a versioned distribution manifest binding every file to a content digest.

The distribution identity contains at least:

- manifest schema version;
- exact Pop Lang toolchain version and release identity;
- source revision used to produce the release;
- host platform target and supported compilation platform targets;
- supported language editions and manifest schema ranges;
- compiler metadata, `.poplib`, documentation, bytecode, and cache schema
  versions where applicable;
- PLRI ABI range, intrinsic-table version, and foundational Bubble identities;
- every file's normalized path, type, size, executable policy, and SHA-256
  digest;
- declared host capabilities that are not included in the distribution.

An exact release and host target are immutable. Published bytes, manifests, or
signatures cannot be replaced under the same identity. A correction receives a
new release identity.

Every installed distribution is relocatable as a directory tree. Executables
locate shipped assets relative to the selected toolchain root or its verified
manifest. They never embed a source checkout, Cargo target directory, developer
home, or installation prefix. Normal use cannot invoke Cargo to rebuild missing
toolchain components. Any required host linker or operating-system capability
is declared in the manifest, checked before use, and reported through a
structured diagnostic rather than discovered through an unrelated build
failure.

### Signed release metadata

The official repository and release service are transports, not semantic or
security identity. `popup` does not scrape HTML, treat Git tags as executable
release metadata, infer assets from filenames, or install an arbitrary branch
tip.

Release discovery uses canonical, versioned, signed metadata:

1. a trusted root defines key identifiers, Ed25519 public keys, signature
   thresholds, supported metadata algorithms, and expiry;
2. a signed release index has a monotonically increasing sequence number,
   schema version, expiry, exact release records, and the signed `stable`
   pointer;
3. each release record binds an exact version and host target to its immutable
   distribution-manifest URL, byte size, and SHA-256 digest;
4. the distribution manifest binds the archive and all installed files.

The official origin is fixed by the accepted distribution policy. A mirror is
used only through explicit configuration and must serve metadata and artifacts
verifiable by the same trusted root. Redirects, cross-origin requests, proxy
configuration, and credentials are handled as typed transport policy; secrets
never enter diagnostics, state files, URLs written to logs, or cache keys.

TLS protects transport, but successful TLS or a hash obtained from the same
unsigned response is not sufficient artifact trust. `popup` verifies the
signature chain, metadata expiry, monotonic sequence, exact host target, byte
size, archive digest, distribution manifest, and file inventory before an
installation can become active.

The manager persists the greatest accepted root and release-index sequence.
Older metadata is rejected as rollback even when correctly signed. Expired
metadata cannot discover, install, update, or change a channel selection.
Already installed, previously verified exact toolchains remain runnable while
offline. An offline operation may use only cached metadata and artifacts that
still satisfy the operation's trust requirements; it never weakens verification
because the network is unavailable.

Root rotation requires threshold authorization by the currently trusted root
and the replacement root. Revocation and emergency recovery procedures are
documented and tested before the first production release. Security metadata
failure is closed: no flag disables signatures, digest verification, expiry, or
rollback protection.

### Bash bootstrap trust boundary

The repository publishes one inspectable Bash script whose only purpose is to
bootstrap `popup`. The script is not the long-term update engine and does not
implement release-channel parsing.

The script:

- supports only an explicit, documented initial host matrix;
- contains a fixed official origin, an exact bootstrap `popup` version, the
  expected host artifact sizes and SHA-256 digests, and the trusted-root
  fingerprint expected by that `popup` version;
- downloads bytes to a private temporary directory under `umask 077`;
- verifies host selection, byte size, and the pinned digest before executing or
  installing anything;
- stages and atomically activates `popup`, then lets the verified manager obtain
  and validate signed release metadata;
- uses strict error handling and cleanup traps;
- never uses `eval`, sources downloaded text, clones or builds the repository,
  runs Cargo, requests `sudo`, writes outside the selected user installation
  root, or edits shell startup files without a separate explicit user action;
- prints a manual download-and-verify alternative and the exact path change
  needed after installation.

The required Bash and digest/download utilities are part of the published host
support contract. If the host cannot provide them, the script stops before
mutation and points to the manual installation path. A convenient
`curl ... | bash` invocation does not remove the user's need to trust the script
bytes delivered by the documented official origin; documentation must make
that boundary explicit rather than claiming end-to-end verification before the
script itself has been obtained.

### Installed state and selector precedence

`popup` owns one versioned state root selected by an explicit `POPUP_HOME` or a
documented platform default. The root contains:

```text
bin/
    popup
    pop
toolchains/
downloads/
state/
staging/
```

`bin/pop` is a small managed shim, not a compiler copy. It resolves one exact
installed toolchain, preserves every argument and process result, and executes
that toolchain's real `pop`. It performs no package resolution and no dynamic
runtime symbol lookup.

Selection uses exactly this precedence:

1. `popup run --toolchain <selection> -- ...`;
2. the explicit `POPUP_TOOLCHAIN` environment override;
3. the nearest ancestor `pop-toolchain.toml` from the canonical working
   directory;
4. the global default recorded by `popup default`.

The nearest `pop-toolchain.toml` is authoritative. A malformed, unsupported, or
unavailable nearest file is an error; lookup does not skip it in search of a
more distant usable file. Ancestor discovery affects toolchain selection only
and never changes Package/Workspace membership or visibility.

The versioned pin format records an exact toolchain version and distribution
digest. A checked-in pin cannot contain `stable` or another moving selector.
Channels are accepted only for an explicit interactive installation/default
operation and resolve to an exact verified release before state is changed.
Selection never downloads implicitly; a missing selected toolchain reports the
exact explicit install command.

### Transaction, concurrency, and recovery model

Downloads, verification, extraction, installation, selection changes,
uninstallation, and self-update are transactions:

1. acquire a scoped state lock without holding it across avoidable network
   waits;
2. download into content-addressed staging with bounded sizes;
3. verify metadata, archive, target, and complete inventory;
4. extract into a new private directory on the destination filesystem;
5. reject absolute paths, `..` traversal, path-prefix collisions, duplicate
   entries, symlinks, hard links, devices, sockets, unexpected file types,
   undeclared files, and unsafe permissions;
6. synchronize required files and atomically rename the immutable directory;
7. atomically replace the small versioned state record or shim;
8. release the lock and remove unreachable staging data.

Failure, cancellation, process termination, disk exhaustion, or a second
manager process must leave the previously active manager, toolchains, and
default selection usable. Recovery is idempotent and distinguishes a verified
complete install from abandoned staging. It never marks partial content active.

Self-update stages a complete verified `popup`, preserves a known-good manager,
and uses a platform-appropriate atomic handoff. The running executable is never
overwritten in place. A failed first start rolls back to the known-good manager.
Uninstall refuses to remove a selected, pinned, or currently leased toolchain
unless an explicit safe selection change makes it unreachable first. Cache
collection cannot remove installed or transaction-referenced content.

No background task silently updates the manager, channel metadata, default
selection, or compiler. Network access and state mutation occur only for an
explicit `popup` command.

### Structured diagnostics and events

`popup` uses the shared diagnostic catalog, typed arguments, severity, message
keys, notes, ordering, human renderer, and versioned machine schemas. Toolchain
diagnostics use stable non-suppressible codes in the reserved `POP9000–POP9999`
range.

The shared model must support a typed diagnostic location rather than requiring
a fabricated source span. Initial location variants are source span, manifest
key/range, filesystem path, artifact plus inventory entry, toolchain state, and
no location. `popup` uses only the variants supported by the actual failure.
Source-oriented compiler diagnostics retain their precise source spans.

At minimum the catalog distinguishes unsupported host, unavailable release,
offline cache miss, signature failure, digest mismatch, expired metadata,
rollback attempt, incompatible PLRI or foundational Bubbles, unsafe archive,
corrupt state, concurrent transaction, insufficient space, permission failure,
and self-update recovery. Trust failures, toolchain invariant failures, and
architecture incidents cannot be suppressed, demoted, or fixed by disabling
verification.

Progress is a separate typed, versioned event stream. Neither the TUI nor
machine clients scrape human diagnostic or progress text. Cancellation emits no
success event and publishes no partial state.

The Bash bootstrap cannot use the Rust diagnostic implementation before it has
installed `popup`; it provides concise human errors and stable documented exit
classes without claiming to emit compiler `Diagnostic` objects.

### Plain, JSON, and Ratatui presentation

Every manager operation is implemented once behind a typed command/event/state
API. Plain terminal output, versioned JSON, and the Ratatui UI consume that same
API and must agree on selected versions, available actions, diagnostics,
confirmation requirements, progress, and final results.

The TUI is an optional presentation, never the only way to install, select,
update, recover, or inspect toolchains. It starts only when explicitly requested
or when the documented no-argument invocation has interactive input and output.
A pipe, redirected stream, continuous-integration environment, JSON message
format, or incapable terminal never enters alternate-screen mode.

Color and animation are optional and never carry meaning. Plain text and JSON
remain complete without them. The UI preserves deterministic ordering, exposes
keyboard-readable labels and confirmations, supports cancellation, and restores
terminal mode, cursor, and alternate screen after normal completion, error,
panic containment, or supported termination signals.

JSON is a versioned schema of typed results, diagnostics, and events. Human
wording, glyphs, spacing, color, screen coordinates, and widget structure are
not machine APIs.

Ratatui and its terminal backend are confined to the `popup` presentation
crate. Toolchain identity, trust, selection, transaction, and recovery code has
no terminal dependency and can be tested with deterministic adapters.

### Rust crate and dependency boundaries

On acceptance, ADR 0018's member inventory is amended with focused boundaries:

```text
tools/
  toolchain/       identities, metadata trust, selection, state, transactions
  popup/           `popup` binary, transport/filesystem adapters, plain/JSON/TUI
```

The packages use the normal `pop-` Cargo prefix. The `popup` package emits the
binary named `popup`; the toolchain package is a library and has no UI policy.
The bootstrap script lives under `scripts/` and remains Bash, not Python.

Ratatui, terminal, HTTP/TLS, serialization, semantic-version, archive, digest,
and signature libraries require explicit version, feature, license, maintenance,
and security review before they are added centrally to the workspace. The
project does not implement cryptographic primitives or TLS ad hoc to avoid a
dependency. Architecture tests confine each approved dependency to its owning
boundary and continue to prevent terminal/network/distribution policy from
entering HIR, MIR, backends, the runtime interface, or Pop's base libraries.

### Release pipeline

A production release pipeline:

1. accepts an exact release identity and immutable source revision;
2. runs formatting, compiler, runtime, architecture, diagnostic, documentation,
   and cross-backend gates before packaging;
3. builds every declared host distribution from reviewed locked inputs;
4. verifies that release executables and libraries contain no checkout or build
   directory dependency;
5. reproduces each distribution in an independent clean build and compares the
   normalized manifest and artifact digests;
6. emits license inventory, software bill of materials, distribution manifest,
   archive, hashes, and provenance;
7. signs release records in a protected stage separated from compilation;
8. uploads immutable artifacts and verifies them from the public transport;
9. smoke-installs into an empty user root outside the source checkout, with
   Cargo unavailable, then runs `popup doctor`, `pop` compiler checks, runtime
   execution, and foundational Bubble verification;
10. publishes the new signed release index and `stable` pointer only after every
    referenced host artifact is present and verified.

A partially uploaded release is never discoverable through signed channel
metadata. Publication cannot replace an existing release/target asset. Revoking
or withdrawing a release updates signed metadata without deleting the evidence
needed to diagnose existing installations. Existing exact installations remain
identifiable; policy decides whether execution is warned or refused based on a
signed security status, never on an unsigned repository label.

The supported host matrix is a release contract and contains only targets whose
complete compiler/runtime/linker behavior passes the installation smoke test.
Repository build capability alone does not claim distribution support.

## Security and reproducibility invariants

- Repository names, tags, release titles, asset filenames, and paths are display
  or transport facts, never toolchain security identity.
- `stable` cannot change an already running command or mutate a checked-in exact
  pin.
- An installation becomes selectable only after complete verification and
  atomic activation.
- Offline mode never degrades signature, digest, expiry, rollback, or target
  checks.
- Toolchain selection never widens Pop Lang visibility, changes Package/Bubble
  identity, or bypasses `bubble.lock` dependency resolution.
- The selected exact compiler/toolchain identity remains part of build/cache
  identity and machine metadata.
- The manager never executes downloaded scripts, repository source, Package
  build hooks, or unverified toolchain content.
- No diagnostic, progress event, state file, crash report, or cache key contains
  credentials.
- TUI state and color do not define command semantics.
- HIR and MIR remain backend-neutral and contain no installer, release, terminal,
  or transport concepts.

## Consequences

- Users can install and select multiple Pop Lang toolchains without confusing
  compiler distributions with published Packages or binary Bubbles.
- Exact checked-in toolchain pins make compiler selection reproducible while a
  signed `stable` channel remains convenient for interactive use.
- The release service and official repository can be replaced as transports
  without changing immutable distribution identity or weakening verification.
- Relocatable releases require the current bootstrap driver to stop depending
  on repository-local Cargo outputs and undeclared developer tools.
- Strong supply-chain verification, recovery, and cross-platform atomicity add
  implementation and release-engineering cost.
- Ratatui improves interactive discovery and selection but remains isolated from
  toolchain semantics and machine protocols.
- `popup` creates a carefully bounded exception to the unified `pop` command,
  not a second Package manager.

## Alternatives considered

### Put toolchain management under `pop install`

Rejected because Package binary Bubbles and compiler/runtime distributions have
different identities, trust roots, compatibility contracts, and recovery needs.
It would also require a working `pop` before the toolchain was installed.

### Add `pop toolchain` without a separate executable

Rejected for bootstrap and self-replacement. A small manager/shim can remain
usable while an individual compiler distribution is absent, broken, or being
replaced.

### Scrape repository tags or release pages

Rejected because presentation and Git state do not provide a versioned schema,
complete target inventory, expiry, rollback protection, or independent artifact
verification.

### Download and replace one mutable `pop` binary

Rejected because the compiler, runtime, foundational Bubbles, PLRI, and shipped
tools are a compatibility set. In-place replacement also makes interrupted
updates destructive and directory-specific selection impossible.

### Make the Ratatui interface the only workflow

Rejected because scripts, continuous integration, accessibility, recovery, and
machine integrations require deterministic noninteractive commands and
versioned JSON.

### Trust TLS and an adjacent checksum file alone

Rejected because one compromised transport/origin could replace both bytes and
checksum, and neither mechanism provides metadata expiry, rollback protection,
threshold key rotation, or immutable release identity.

## Required conformance tests before implementation

Acceptance requires tests to be added before production implementation and
observed failing for the missing behavior.

### Architecture and dependency tests

- accepted Cargo member inventory includes only the two approved new boundaries;
- the `popup` binary name and toolchain library targets are exact;
- Ratatui and terminal dependencies occur only in the `popup` crate;
- transport/archive/cryptographic dependencies occur only in approved
  distribution boundaries;
- portable compiler, HIR, MIR, backends, runtime interface, and base libraries
  have no `popup`, Ratatui, network, archive, or release-metadata dependency;
- every new external dependency matches the accepted version/features/license
  baseline and the Cargo lock is reproducible.

### Metadata, version, and trust tests

- canonical root, release-index, release-record, and distribution-manifest
  fixtures round-trip without changing signed bytes;
- unknown critical fields, duplicate fields/keys, malformed versions, invalid
  targets, noncanonical encodings, oversized integers/strings/collections, and
  unsupported schema/algorithm versions are rejected;
- stable versions and preview versions sort deterministically; preview versions
  are hidden by default;
- `stable` resolves exactly to its signed release record and records the exact
  version/digest;
- Git tags, release titles, HTML, branches, and asset filenames cannot create or
  alter a release record;
- valid threshold signatures succeed; missing, unknown, duplicate, malformed,
  wrong-key, and insufficient-threshold signatures fail;
- wrong archive size, SHA-256 digest, manifest digest, inventory digest, PLRI,
  foundational Bubble identity, or host target fails closed;
- expired/future/rolled-back/frozen metadata and replayed lower sequence numbers
  fail without mutating trusted state;
- valid root rotation requires the accepted old/new thresholds; revoked or
  unauthorized keys cannot rotate trust;
- redirects, mirrors, proxies, credential-bearing URLs, and secret redaction
  follow the exact transport policy;
- offline use runs installed exact toolchains but cannot invent, refresh, or
  weaken release trust.

### Archive and filesystem tests

- absolute, parent-traversal, separator-confusion, Unicode/path-normalization,
  case-collision, prefix-collision, and destination-escape entries fail;
- symlink, hard-link, device, socket, FIFO, duplicate, undeclared, oversized,
  truncated, and permission-escalating entries fail;
- extraction never writes outside private staging and never executes content;
- the final inventory contains exactly the declared normalized files, sizes,
  types, modes, and digests;
- relocatable tools find all shipped assets after moving the complete directory
  and with the repository, Cargo, and Cargo target directory unavailable.

### Selection and command tests

- explicit `popup run`, environment, nearest exact pin, and global default obey
  the specified precedence;
- malformed/unavailable nearest pins fail rather than falling through;
- channel selectors are rejected in checked-in pins and resolve exactly before
  interactive state mutation;
- missing selected toolchains report an explicit install command and never
  download implicitly;
- the shim preserves arguments, standard streams, signals, working directory,
  environment, and process exit status;
- `pop install` continues to install a Package binary Bubble and never invokes
  toolchain management;
- `popup` never edits `bubble.toml`, `bubble.lock`, source, Package artifacts, or
  Workspace selection;
- list/install/default/run/update/uninstall/doctor/self-update positive,
  negative, help, invalid-option, and idempotence cases are deterministic.

### Transaction and recovery tests

- failure injection before and after every download, verification, extraction,
  synchronization, rename, state swap, and cleanup step preserves the prior
  usable state;
- cancellation, termination, disk exhaustion, permission failure, corrupt
  staging, and process restart recover idempotently;
- concurrent installs of the same/different releases and concurrent
  install/select/uninstall/self-update operations serialize without deadlock,
  lost state, or partial activation;
- an existing immutable release cannot be replaced by different bytes;
- self-update never overwrites the running executable and rolls back after a
  failed first start;
- selected, pinned, or leased toolchains cannot be removed, and cache cleanup
  preserves all reachable content;
- no background or ordinary `pop` invocation performs network access or state
  mutation.

### Bootstrap tests

- every supported host maps to one exact pinned manager artifact and every
  unknown host fails before download or mutation;
- the script uses private temporary storage, restrictive permissions, strict
  failure handling, and cleanup traps;
- wrong size/digest, truncated download, redirect-policy failure, unavailable
  digest utility, interrupted download, and unwritable destination leave no
  active or executed partial manager;
- no downloaded byte executes before verification;
- fixtures prove the script never uses `eval`, sources network content, clones
  or builds the repository, invokes Cargo, requests `sudo`, or silently edits
  shell configuration;
- repeated installation is idempotent and preserves a known-good manager on
  failure;
- the documented manual verification path selects the same bytes and digest.

### Diagnostic, plain, JSON, and TUI tests

- each required `POP9xxx` diagnostic has typed arguments, intrinsic severity,
  catalog documentation, stable ordering, and is non-suppressible where
  required;
- toolchain/path/artifact/no-location diagnostics use no fabricated `FileId` or
  `SourceSpan`;
- human and JSON golden tests contain the same diagnostic and result facts;
- plain, JSON, and TUI adapters consume the same scripted typed operation/event
  trace and reach the same final state;
- non-TTY, redirected, continuous-integration, and JSON invocations never enter
  alternate-screen mode or emit control sequences;
- color-disabled and limited-color output remains complete and distinguishable;
- terminal resizing, cancellation, renderer failure, panic containment, and
  supported termination signals restore cursor, terminal mode, and screen;
- TUI ordering, focus, labels, confirmation, keyboard navigation, progress, and
  errors are deterministic and accessible without color;
- machine clients never need to parse human strings, glyphs, progress bars, or
  widget layout.

### Release-pipeline and end-to-end tests

- clean independent builds produce the same normalized distribution manifest
  and artifact digests;
- inventories contain the complete expected compiler, runtime, foundational
  Bubble, tools, documentation, licenses, and declared host capabilities;
- binaries contain no checkout, developer-home, Cargo target, or installation
  prefix dependency;
- every supported host installs into an empty root and runs `popup doctor`,
  compiler checking, native/runtime execution, foundational Bubble loading, and
  diagnostic rendering outside the checkout;
- backend/runtime conformance for the installed distribution matches the same
  source tests run from the build tree;
- an index cannot be published before every referenced artifact is publicly
  available and reverified;
- a partial upload, duplicate release identity, asset replacement, failed smoke
  test, missing license/SBOM/provenance, or signing failure blocks publication;
- withdrawn/revoked release status is signed, deterministic, and cannot be
  forged by an unsigned repository label;
- the installed exact toolchain version/digest appears in build metadata and
  cache identity.

## Documents and components affected on acceptance

- architecture overview and implementation roadmap;
- closed toolchain/package design questions;
- compiler component and Rust Cargo workspace architecture;
- Bubble artifact/loading and foundational-library trust contracts;
- diagnostics, rendering, machine schemas, and `POP9xxx` catalog documentation;
- architecture conformance policy, traceability matrix, and release gates;
- CLI/tooling architecture, explicitly distinguishing `popup` from
  `pop install`;
- repository README, installation, manual verification, security, key rotation,
  platform support, uninstall, and recovery documentation;
- root Cargo manifests/lockfile and architecture member/dependency tests;
- compiler driver, runtime, base-library, linker, and tool asset lookup needed
  for relocatable distributions;
- bootstrap script, release schemas/fixtures, release workflow, signing service,
  and end-to-end installation tests.

Until this proposal is accepted and those documents/tests are synchronized,
`popup`, its release formats, and its dependencies remain unauthorized
architecture-gap work rather than stable Pop Lang behavior.
