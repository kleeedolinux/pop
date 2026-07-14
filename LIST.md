# Pop Lang Batteries List

Proposed first-party batteries for a modern Pop Lang ecosystem. Libraries should
be portable by default, use explicit platform adapters where required, and avoid
linking unused families. This is an inventory, not an implementation-status list.

## Family index

- Core and portable: `Archive`, `Bytes`, `Codec`, `Compress`, `Csv`, `Glob`,
  `Guid`, `Json`, `Locale`, `Math`, `Mime`, `Random`, `Regex`, `Resource`,
  `Sequence`, `Text`, `Time`, `Toml`, `Unicode`, `Uri`, `Version`, `Xml`, and
  `Yaml`.
- System, network, and security: `Actor`, `Atomic`, `Channel`, `Cluster`,
  `Crypto`, `Device`, `Directory`, `Environment`, `File`, `Http`, `Identity`,
  `Io`, `Memory`, `Net`, `Path`, `Platform`, `Process`, `Schedule`, `Socket`,
  `Task`, `Terminal`, and `WebSocket`.
- Data, observability, and tooling: `Benchmark`, `Cli`, `Command`, `Data`,
  `Diagnostic`, `Documentation`, `Lsp`, `Metadata`, `Package`, `Settings`,
  `Source`, `Sql`, `Store`, `Syntax`, `Telemetry`, and `Test`.
- Applications, media, and science: `Ai`, `Audio`, `Email`, `Ffi`, `Geometry`,
  `Graphics`, `Image`, `Media`, `Message`, `Rpc`, `Science`, `Signal`,
  `Statistics`, `Tensor`, `Ui`, `Units`, and `Video`.

## Language foundation

- Primitive integers, floating-point values, decimals, bytes, booleans, strings,
  and the `Never` type.
- Optional values, typed `Result` errors, tuples, records, tagged unions, enums,
  ranges, and fixed multiple results.
- Equality, ordering, hashing, copying, conversion, parsing, formatting, and
  checked casts.
- Fixed arrays, growable lists, maps, sets, tables, queues, stacks, double-ended
  queues, priority queues, heaps, bit sets, and immutable collection variants.
- Collection views, slices, spans, borrowed windows, reusable buffers, and
  allocation-aware bulk operations.
- `Iterable<T>`, `Iterator<T>`, lazy sequences, ranges, mapping, filtering,
  folding, collecting, zipping, chunking, windowing, grouping, sorting,
  searching, partitioning, aggregation, and parallel operations.
- First-class functions, closures, coroutines, tasks, cancellation, and scoped
  cleanup support.

## Mathematics and numerics

- Checked, saturating, wrapping, overflowing, and arbitrary-precision integer
  arithmetic.
- Complete floating-point functions: rounding, decomposition, powers,
  logarithms, roots, trigonometry, hyperbolic functions, classification, and
  correctly documented exceptional values.
- Decimal arithmetic with explicit precision, scale, rounding, and conversion.
- Big integers, rational numbers, complex numbers, and modular arithmetic.
- Number theory: greatest common divisors, primality, factorization helpers,
  modular powers, residues, and combinatorics.
- Common constants, special functions, interpolation, polynomials, and robust
  comparison helpers.
- Vectors, matrices, quaternions, transforms, dense and sparse linear algebra,
  decompositions, and equation solvers.
- Numerical integration, differentiation, root finding, optimization, and
  ordinary differential equations.
- Deterministic pseudo-random generators with documented streams and seeds.
- Probability distributions, weighted sampling, shuffling, and reproducible
  simulation helpers.
- Cryptographically secure randomness kept separate under `Crypto`.

## Text and Unicode

- UTF-8 strings, non-owning text views, rune iteration, grapheme iteration, and
  safe slicing that never splits an encoded character.
- Unicode normalization, casing, categories, scripts, properties, width, and
  grapheme, word, sentence, and line segmentation.
- Split, join, trim, replace, search, compare, prefix, suffix, padding, wrapping,
  indentation, and line-ending normalization.
- Typed parsing and formatting for numbers, booleans, dates, times, durations,
  identifiers, and application values.
- String interpolation and typed formatting with explicit locale behavior.
- Escaping and unescaping for source text, shells, URLs, HTML, XML, JSON, CSV,
  regular expressions, and terminal output.
- Reusable text and byte buffers with checked UTF-8 completion.
- Text diffs, tokens, templates, source maps, and safe typed substitutions.
- Regular expressions with bounded execution, captures, replacement, and a
  streaming-search subset.
- Globs for text and paths with deterministic, bounded expansion.

## Bytes, binary data, and codecs

- Owned byte arrays, borrowed byte views, reusable buffers, endian reads and
  writes, bit operations, and binary cursors.
- Hexadecimal, Base16, Base32, Base58, Base64, Base64Url, and ASCII armor.
- Checksums and noncryptographic hashes for files, protocols, and data
  structures.
- Typed encoders and decoders, schema descriptors, generated adapters, framing,
  incremental parsing, and streaming codecs.
- Canonical and deterministic encoding modes where formats support them.
- Explicit depth, size, recursion, allocation, and expansion limits.
- First-party adapters for CBOR, MessagePack, Protocol Buffers, BSON, ASN.1, and
  other widely used binary interchange formats.

## Data formats and configuration

- JSON encoding, decoding, streaming events, JSON Lines, typed schemas, and
  deliberate schema-less `Json.Value` support.
- YAML safe-mode encoding and decoding, typed schemas, event streams, aliases,
  and bounded expansion.
- XML encoding and decoding, namespaces, pull events, bounded trees,
  canonicalization, and schema adapters, with unsafe entity resolution disabled.
- CSV and TSV with typed rows, configurable dialects, streaming input and
  output, formula-injection handling, and reusable row buffers.
- TOML typed decoding, encoding, validation, and source-preserving edits.
- INI as a first-party compatibility package for ecosystems that require it.
- MIME types, parameters, matching, extension lookup, and bounded content
  sniffing.
- URI and URL parsing, normalization, resolution, percent encoding, query
  values, and internationalized host handling.
- Semantic versions, version requirements, comparisons, and compatibility
  ranges.
- GUID and UUID parsing, formatting, generation, and version-specific values.
- Typed settings merged from defaults, files, environment, arguments, and
  secret sources.

## Compression and archives

- Streaming Deflate, Gzip, Zstandard, Brotli, LZ4, XZ, Bzip2, and Snappy
  compression where licensing and target support permit them.
- ZIP, TAR, compressed TAR, and 7-Zip archive reading and writing.
- Safe extraction with path traversal, link, entry-count, expanded-size, and
  compression-bomb limits.
- Random-access and streaming archive APIs, metadata preservation, and
  caller-provided buffers.
- Content-addressed blobs, checksums, and integrity verification.

## Time, locale, and resources

- Monotonic instants, wall-clock times, durations, deadlines, timers,
  stopwatches, and injected test clocks.
- Dates, local date-times, offsets, time zones, daylight-saving transitions,
  calendars, and versioned time-zone data.
- Parsing and formatting with explicit locale and time-zone inputs.
- Locale tags, negotiation, collation, number formatting, currencies, units,
  calendars, plural rules, and select rules.
- Typed localized messages with compile-time checked keys and parameter records.
- Packaged text and binary resources, locale fallback, generated resource keys,
  themes, templates, and versioned data packs.
- Durable schedules, calendar jobs, retry policies, misfire policies, test
  schedules, and host scheduling adapters.

## I/O, paths, files, and directories

- Typed readers, writers, streams, seeking, buffering, copying, pipes,
  backpressure, and caller-owned buffers.
- Portable lexical paths with native encoding preservation, normalization,
  joining, relative resolution, components, names, extensions, and parents.
- File open, create, read, write, append, truncate, atomic replacement, flush,
  synchronization, locking, memory mapping, metadata, permissions, and links.
- Temporary files and directories with deterministic cleanup.
- Directory create, remove, list, walk, glob, watch, metadata, and standard
  user/system locations.
- Explicit filesystem capabilities, rooted access, virtual filesystems, and
  in-memory filesystems for tests.
- Safe handling of symbolic links, traversal, permissions, encodings, large
  files, sparse files, and partial I/O.

## Memory and low-level data

- Bounds-checked memory views, typed spans, arenas, pools, slabs, ring buffers,
  mapped buffers, and shared mappings.
- Explicit allocation, retention, alignment, ownership, copying, filling, and
  zeroing behavior.
- Secure secret storage with redaction and best-effort zeroization.
- Explicit unsafe raw addresses, native pointers, pinning, and ABI memory only
  under `Memory.Unsafe` or `Ffi.Unsafe`.

## Processes, environment, and platform

- Process arguments, executable paths, working directories, exit status,
  resource limits, and signals.
- Process spawning, waiting, killing, pipelines, bounded output capture, typed
  interprocess communication, and sandbox profiles.
- Shell execution only through a visibly unsafe or explicitly quoted API.
- Immutable environment snapshots, typed environment decoding, child-process
  overrides, secret redaction, and target-aware encoding.
- Typed operating-system, architecture, runtime, toolchain, feature, and
  capability facts.
- Linux, Windows, macOS, Android, iOS, POSIX, WebAssembly, and web-host adapters.
- Native threads, processor facts, affinity, priorities, host services/daemons,
  notifications, and platform integration where available.
- Standard streams, terminals, colors, styles, cursor control, input events,
  password input, prompts, line editing, progress, and tables.

## Concurrency and asynchronous work

- Structured tasks, task groups, joining, racing, selection, yielding, sleeping,
  deadlines, cancellation, and exact failure propagation.
- Bounded and unbounded typed channels with sender/receiver separation and
  selection support.
- Locks, read/write locks, semaphores, events, barriers, latches, condition
  variables, and scoped task-local values.
- Worker pools, parallel sequences, deterministic schedulers, and test
  executors.
- Atomic integers, booleans, safe handles, memory ordering, fences, and
  wait/notify.
- Isolated local actors with typed mailboxes, replies, monitors, supervision,
  restart policies, shutdown, and bounded admission.
- Distributed actors and clusters with authenticated endpoints, placement,
  membership, health, typed delivery outcomes, and explicit partial failure.

## Networking

- IPv4, IPv6, addresses, prefixes, interfaces, routing facts, and network-change
  observation.
- DNS lookup, reverse lookup, records, caching, resolver configuration, DNS over
  TLS, and DNS over HTTPS.
- TCP, UDP, Unix-domain sockets, multicast, broadcast, socket options, and raw
  sockets behind explicit unsafe capability.
- Typed socket handles, reusable buffers, deadlines, cancellation, half-close,
  keepalive, and connection state.
- TLS clients and servers, certificate validation, mutual TLS, session reuse,
  protocol negotiation, and curated cipher policy.
- QUIC streams and datagrams with explicit flow control and connection state.
- Proxies, tunneling, connection pools, retry/backoff values, rate limits, and
  in-memory test transports.
- HTTP/1.1, HTTP/2, and HTTP/3 clients and servers.
- HTTP requests, responses, methods, statuses, headers, trailers, cookies,
  forms, multipart data, compression, caching, proxies, authentication,
  redirects, retries, routing, middleware functions, and streaming bodies.
- WebSocket handshake, frames, typed messages, ping/pong, close, compression,
  limits, and backpressure.
- Server-sent events and streaming HTTP events.
- SSH and SFTP clients, servers, keys, agents, known-host verification, and
  port forwarding as explicit official packages.
- Local and network test servers, deterministic transports, fault injection,
  packet fixtures, and protocol fuzzing helpers.

## Cryptography and security

- Operating-system secure randomness, deterministic test randomness, secret
  byte values, constant-time comparison, and redacted formatting.
- Cryptographic hashes: SHA-2, SHA-3, BLAKE2, BLAKE3, and extendable-output
  functions where appropriate.
- Message authentication: HMAC, Poly1305, and algorithm-agile typed MAC values.
- Key derivation: HKDF, PBKDF2, scrypt, Argon2, and memory/iteration policies.
- Symmetric encryption: AES modes required by modern protocols and ChaCha20.
- Authenticated encryption: AES-GCM, AES-GCM-SIV, ChaCha20-Poly1305, and
  XChaCha20-Poly1305.
- Public-key signatures: Ed25519, ECDSA, RSA-PSS, and protocol-required legacy
  verification kept separate from safe signing defaults.
- Key exchange: X25519, ECDH, hybrid/post-quantum-ready typed negotiation, and
  protocol-specific key schedules.
- Password hashing, password verification, password generation, secret sharing,
  and encrypted key storage.
- Key generation, import, export, rotation, identifiers, fingerprints, and
  hardware/platform key-store adapters.
- PEM, DER, JWK, PKCS formats, X.509 certificates, certificate chains,
  certificate requests, revocation data, and trust stores.
- Signed and encrypted tokens, JWT, PASETO-style tokens, timestamped signatures,
  and content signatures.
- Authenticated streaming and file encryption formats built only from reviewed
  primitives.
- Curated safe defaults, explicit algorithm/version selection, compliance
  profiles, test vectors, and an isolated `Crypto.Unsafe` legacy surface.
- No home-grown cryptographic primitives or insecure compatibility defaults.

## Identity, permissions, and secrets

- Principals, typed claims, credentials, authentication results, sessions, and
  explicit current-user inputs.
- OAuth 2, OpenID Connect, JWT validation, device flows, service credentials,
  and token refresh.
- Passkeys, WebAuthn, one-time passwords, recovery codes, and multifactor flows.
- Permissions, roles, policies, capability values, and typed authorization
  decisions.
- Platform credential stores, hardware security modules, key stores, secret
  stores, and redacted configuration.
- Certificate identity, workload identity, federation, and service-to-service
  authentication.

## Databases, storage, and data processing

- Typed rows, columns, schemas, datasets, queries, nullability, data frames, and
  bulk transforms.
- SQL connections, pools, prepared statements, parameters, cursors,
  transactions, savepoints, migrations, schema inspection, and typed queries.
- First-party adapters for SQLite, PostgreSQL, MySQL/MariaDB, and other major
  engines selected by explicit package.
- Key-value, document, graph, time-series, object/blob, embedded, and cloud
  storage contracts.
- First-party adapters for common caches, embedded stores, document stores,
  message logs, and cloud object stores.
- Streaming rows and blobs, batching, pagination, retries, cancellation,
  timeouts, isolation, and explicit consistency levels.
- In-memory stores, deterministic database fakes, fixtures, migrations, and
  compatibility suites.
- Data validation, schema evolution, import/export, deduplication, joins,
  grouping, sorting, aggregation, and columnar interchange.

## Telemetry and operations

- Structured logs with typed fields, levels, filtering, scopes, correlation,
  sampling, buffering, and redaction.
- Distributed traces with spans, links, propagation, sampling, baggage, and
  explicit exporters.
- Metrics with counters, gauges, histograms, summaries, units, labels, and
  aggregation.
- Crash reports, stack/source information, breadcrumbs, and privacy controls.
- OpenTelemetry-compatible interchange and first-party console, file, OTLP,
  test, and platform exporters.
- Health, readiness, liveness, diagnostics, resource facts, and structured
  application events.
- Profiling hooks for CPU, allocation, tasks, I/O, locks, and native
  transitions.
- Deterministic capture sinks for tests and near-zero disabled-path overhead.

## Command-line applications and settings

- Typed commands, subcommands, options, flags, positional arguments, validation,
  defaults, environment inputs, and configuration files.
- Generated parsers, help, usage, shell completion, manual pages, error
  diagnostics, and exit-code mapping.
- Prompts, secret input, confirmation, selection, progress, spinners, tables,
  colors, and redirected-output fallbacks.
- Typed immutable settings, layered sources, live reload, validation, secret
  sources, and explicit precedence.
- Application directory conventions, state/cache paths, lock files, and
  single-instance coordination.

## Testing, benchmarking, and quality

- Assertions, typed comparisons, expected errors, parameterized cases, fixtures,
  setup, cleanup, filtering, and structured test results.
- Property testing, generators, shrinking, deterministic seeds, state-machine
  tests, and model-based tests.
- Snapshot and golden-file tests with structured reviewable differences.
- Fuzzing, corpus management, malformed-input tests, resource-limit tests, and
  sanitizer integration.
- Test clocks, randomness, filesystems, processes, transports, terminals,
  telemetry sinks, databases, and device fakes.
- Unit, integration, documentation-example, conformance, cross-backend, and
  target capability tests.
- Benchmarks with warmup, sampling, statistics, allocation counts, native
  transitions, baselines, comparisons, and machine facts.
- Code coverage, mutation testing, leak detection, race detection, profiling,
  and reproducible test reports.

## RPC, messaging, and application protocols

- Typed request/response, client/server streaming, schemas, generated stubs,
  deadlines, cancellation, authentication, and in-memory transports.
- First-party RPC adapters for JSON-RPC, gRPC, Connect-style protocols, and
  other widely used typed transports.
- Broker-neutral typed envelopes, topics, partitions, acknowledgements,
  delivery results, batches, retries, dead letters, and backpressure.
- First-party AMQP, MQTT, Kafka-compatible, event-stream, and major cloud-queue
  adapters.
- Event sourcing, durable logs, projections, subscriptions, consumer groups,
  and explicit delivery semantics.
- Email addresses, messages, headers, attachments, MIME composition, SMTP,
  IMAP, mailbox operations, and deterministic test transports.
- Calendar and contact interchange packages where application ecosystems need
  them.

## Web and server applications

- HTTP routing, typed path/query/header/body extraction, middleware functions,
  request limits, errors, and generated route metadata.
- Static files, ranges, caching, compression, content negotiation, uploads,
  downloads, cookies, sessions, CSRF protection, CORS, and security headers.
- Typed HTML and safe templating, escaping, localization, forms, validation,
  multipart data, and streaming rendering.
- Authentication and authorization integration through explicit identity
  values.
- Reverse-proxy facts, forwarded headers, graceful shutdown, health endpoints,
  rate limiting, and observability.
- WebAssembly and browser adapters for HTTP, storage, workers, timers, streams,
  cryptography, UI, clipboard, and platform capabilities.

## Images, graphics, fonts, audio, video, and media

- Image pixels, formats, views, color spaces, metadata, decoding, encoding,
  resize, crop, rotate, composite, filter, and conversion.
- First-party image codecs for PNG, JPEG, GIF, WebP, AVIF, BMP, TIFF, and other
  broadly used formats, split when licensing or native dependencies require it.
- Raster and vector graphics, paths, paints, gradients, transforms, clipping,
  canvases, scenes, command buffers, and GPU adapters.
- SVG parsing, rendering, writing, and safe external-resource handling.
- Font discovery, loading, fallback, shaping, glyph runs, rasterization,
  OpenType features, variable fonts, and text layout.
- Audio samples, buffers, formats, channel layouts, decoding, encoding,
  resampling, mixing, processing graphs, capture, playback, and devices.
- First-party codecs and containers for WAV, FLAC, Opus, Vorbis, MP3, AAC, and
  other broadly used formats subject to licensing.
- Video frames, pixel formats, color, timestamps, decoding, encoding, scaling,
  capture, playback, hardware surfaces, and bounded frame pools.
- Media containers, tracks, packets, timelines, seeking, muxing, demuxing,
  streaming sessions, subtitles, and metadata.
- First-party support for MP4, WebM, Matroska, MPEG transport streams, and other
  important containers through explicit codec packages.

## User interfaces

- Immutable views, state/update functions, effects, commands, and composable
  ordinary functions.
- Windows, layout, text, images, lists, tables, forms, menus, dialogs, input,
  focus, navigation, styling, themes, animation, drag/drop, and clipboard.
- Accessibility semantics, keyboard navigation, screen-reader support,
  localization, scaling, color contrast, and reduced-motion behavior.
- Headless rendering and deterministic UI testing.
- Desktop, mobile, web, terminal, and embedded adapters with explicit platform
  capabilities.
- Native controls where useful without making inheritance-based widget trees the
  portable model.

## Geometry, statistics, science, and engineering

- Two-dimensional and three-dimensional points, vectors, rectangles, boxes,
  rays, lines, planes, transforms, paths, intersections, and spatial indexes.
- Dimension-safe units, SI quantities, conversions, uncertainties, and
  locale-aware formatting.
- Descriptive and streaming statistics, probability distributions, sampling,
  hypothesis tests, confidence intervals, regression, and statistical models.
- Typed tensors with shapes, data types, devices, views, broadcasting,
  reductions, dense/sparse operations, linear algebra, and automatic
  differentiation.
- Signal processing with windows, FFTs, filters, convolution, resampling,
  spectral analysis, and streaming buffers.
- Scientific interpolation, integration, differentiation, optimization,
  differential equations, reproducibility, tolerances, and reusable workspaces.
- Data frames, columnar memory, missing values, categorical data, joins,
  grouping, rolling windows, and scientific import/export.
- Geospatial coordinates, projections, geometry, raster/vector data, spatial
  indexes, routes, and common interchange formats.
- Physics, chemistry, biology, finance, engineering, and simulation packages
  with typed units and domain schemas.
- Reproducible notebooks or worksheet tooling built on normal Pop source and
  structured result data rather than a separate dynamic language mode.

## Artificial intelligence and machine learning

- Typed model, tensor, token, input, output, device, and inference-session
  values.
- Model loading, validation, metadata, hashing, versioning, caching, conversion,
  and portable interchange such as ONNX.
- CPU, GPU, accelerator, and remote runtimes through explicit adapters.
- Training loops, optimizers, losses, schedules, checkpoints, mixed precision,
  distributed training, and reproducibility metadata.
- Tokenization, embeddings, vector search, ranking, classification, generation,
  streaming, structured output, and multimodal inputs.
- Typed tool calls, schemas, function identities, validation, timeouts, limits,
  and explicit authority.
- Datasets, batching, shuffling, transforms, evaluation, metrics, experiment
  records, and model comparison.
- Local and remote model adapters without vendor-specific values becoming the
  portable root API.
- Safety filters, policy hooks, redaction, content provenance, and audit events
  as explicit typed components rather than universal guarantees.

## Devices and hardware

- Serial ports, USB, Bluetooth, network discovery, sensors, cameras, location,
  printing, audio devices, video devices, and human-interface devices.
- Device enumeration, permissions, capabilities, connection, configuration,
  hot-plug events, streaming I/O, and deterministic fakes.
- General-purpose input/output, buses, embedded peripherals, and industrial
  protocols through explicit target packages.
- Battery, power, thermal, display, input, accessibility, and notification facts
  where targets expose them.
- GPU and accelerator discovery, memory, queues, command submission, kernels,
  synchronization, and typed capability selection.

## Native interoperability

- C ABI types, layouts, calling conventions, native libraries, symbols,
  callbacks, ownership annotations, and generated bindings.
- C and C++ header/binding generation with target and ABI metadata.
- Platform APIs, GPU APIs, device SDKs, and web-host bindings through explicit
  packages.
- Callback rooting, thread-entry rules, managed/native ownership, pinning,
  cleanup, error translation, and ABI compatibility hashes.
- Safe generated wrappers over reviewed unsafe primitives without dynamic
  fallback into managed calls.

## Compiler, package, and developer tooling

- Immutable source text, line maps, spans, edits, workspace edits, and source
  mapping.
- Versioned tokens, trivia, lossless syntax trees, parsing, visitors, typed
  transformations, formatting, and source-preserving edits.
- Structured diagnostics, stable codes, typed arguments, labels, notes,
  origins, fixes, JSON, SARIF, and terminal rendering.
- Checked XML documentation, semantic links, examples, search indexes, static
  documentation sites, and editor hover data.
- Package manifests, locks, versions, dependency graphs, registries, local/Git/
  registry dependencies, integrity, signing, publishing, and reproducible
  resolution.
- Build, check, run, test, benchmark, documentation, format, lint, fix, add,
  remove, update, tree, metadata, package, and publish commands through `pop`.
- Language server support for completion, hover, signatures, go-to-definition,
  references, rename, semantic tokens, diagnostics, code actions, formatting,
  workspace edits, progress, and cancellation.
- Debugger support for breakpoints, stepping, variables, tasks, actors, threads,
  stack traces, source maps, and native boundaries.
- Profiler support for CPU, memory, allocations, garbage collection, tasks,
  locks, I/O, native transitions, and flame graphs.
- REPL and scratch execution using the same static language, package graph,
  compiler pipeline, and backend semantics as ordinary Pop source.
- Dependency auditing, license reports, vulnerability data, unused-dependency
  detection, API diffs, compatibility checks, and release tooling.
- Code generators and build plugins using typed, versioned, capability-limited
  schemas without source-string injection or runtime reflection.
