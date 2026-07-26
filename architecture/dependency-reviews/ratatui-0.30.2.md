# Ratatui 0.30.2 Dependency Review

- Decision: [ADR 0027](../decisions/0027-ratatui-terminal-presentation.md)
- Review date: 2026-07-26
- Owner: `pop-driver` presentation/orchestration
- Result: approved for the exact ADR 0027 feature set

## Selected surface

The root declaration is exact:

```toml
ratatui = { version = "=0.30.2", default-features = false, features = ["crossterm_0_29"] }
```

Only `pop-driver` inherits it. `cargo tree -p ratatui --target all -e features`
shows the Crossterm 0.29 backend and no Termion, Termwiz, Termina, calendar,
palette, serialization, image, clipboard, shell, network, or plugin feature.
The locked graph contains exactly one `crossterm`, version 0.29.0.

Ratatui and Crossterm types occur only in
`crates/compiler/driver/src/presentation.rs`. Architecture tests scan every
semantic, backend, runtime, library, extension, and tool source boundary and
reject a terminal implementation type outside the driver.

## Reproducibility

`Cargo.lock` records registry sources and checksums for the complete graph. The
direct terminal packages are:

| Package | Version | SHA-256 registry checksum |
| --- | --- | --- |
| `ratatui` | 0.30.2 | `3274ba0a2c5e1bcad2a2005d20f4dc59dad26b2eb0940fb094500dba4099d57d` |
| `ratatui-core` | 0.1.2 | `cbb175c433c8e28a809d1f5773a2ae96e68c0ce40db865cbab1020bf33ae479c` |
| `ratatui-crossterm` | 0.1.2 | `567584a3b0e6a8203c23de40b4861497266725eb5363dbfd18a1edd603cca9f0` |
| `ratatui-widgets` | 0.3.2 | `66e3d19bcc9130ca376277d93b60767ff121ace3be06f5f95f81dd68956407d1` |
| `crossterm` | 0.29.0 | `d8b9f2e4c67f833b660cdb0a3523065869fb35570177239812ed4c905aeff87b` |

Architecture tests pin the root declaration, owning crate, Ratatui/Crossterm
versions, single-Crossterm condition, and absence of alternative terminal
backends/capabilities.

## License review

The enabled all-platform normal/build graph was enumerated with
`cargo tree -p ratatui --target all --edges normal,build`, then matched to Cargo
package license metadata. Every enabled package uses a permissive license
compatible with the repository's `GPL-3.0-only` tool boundary:

- MIT: Ratatui and its three split crates, Crossterm and its Windows adapter,
  Castaway, Compact String, Convert Case, Darling, Derive More, Instability,
  LRU, Mio, Redox Syscall, Strsim, and Strum families;
- MIT or Apache-2.0 variants: allocator API, bitflags, platform/configuration,
  terminal/event support, hashing/collections, proc-macro support, Unicode
  width/segmentation, and Windows support dependencies;
- Zlib: `foldhash` 0.2.0;
- Apache-2.0 or BSL-1.0: `ryu` 1.0.23;
- MIT or Apache-2.0 plus Unicode-3.0: `unicode-ident` 1.0.24;
- Apache-2.0 with LLVM exception, Apache-2.0, or MIT: `linux-raw-sys`,
  `rustix`, and the WASI support crate.

The review uses the license expressions embedded in the locked crate metadata;
no copyleft dependency is linked into the terminal graph.

## Advisory review

`cargo-audit` 0.22.2 scanned the repository lock with the official
[RustSec Advisory Database](https://rustsec.org/) on 2026-07-26:

```text
Loaded 1169 security advisories
Scanning Cargo.lock for vulnerabilities (157 crate dependencies)
exit status: 0
```

The invocation denied vulnerability, unmaintained, unsound, and yanked
warnings. Dependency updates must repeat the exact feature-tree, license,
checksum, single-Crossterm, and RustSec review.
