# Pop Lang benchmark suite

This suite compares equivalent workloads across Pop Lang, Rust, Python,
JavaScript, Lua, LuaJIT, Luau, Ruby, Go, C, D, C#, and Crystal. It measures
only execution of an already prepared command:

- ahead-of-time sources are compiled once before validation, warmups, or timing;
- scripts are passed directly to their installed interpreter or JIT;
- every prepared command must print the workload's exact checksum before it can
  enter a timed sample;
- process startup and final checksum output remain included for every runtime.

The workloads cover recursive calls, a large integer loop, numeric-array
construction and traversal, short-lived allocation churn, and a retained array
of managed objects. The last two cases are useful GC pressure signals for
managed runtimes, but they are not collector-only microbenchmarks: C and Rust
perform equivalent explicit allocations, and optimizer/runtime policies remain
part of what is measured.

Pop Lang result records include `collectorStage` so bootstrap, stable-token
generational, and future production-generational measurements cannot be mixed.

## Run a batch

`bin/benchmark` is executable and defaults to `batch`. A batch prepares the
complete selected matrix, validates checksums, performs warmups, records all
samples, and writes both the machine-readable result and self-contained report.

```bash
benchmark/bin/benchmark
benchmark/bin/benchmark batch --samples 9 --warmups 2
```

From this directory, use `./bin/benchmark` instead. Narrow a batch by repeating
selectors:

```bash
./bin/benchmark batch \
  --runtime poplang --runtime rust --runtime python \
  --workload allocationChurn --workload objectArray
```

For a host-only GC diagnostic without provisioning or Docker, compare the two
checksum-equivalent allocation workloads directly:

```bash
benchmark/bin/benchmark run \
  --runtime poplang --runtime go \
  --workload allocationChurn --workload objectArray \
  --samples 15 --warmups 4 --output /tmp/poplang-gc.json
```

`allocationChurn` fills 20,000 short-lived 256-element numeric arrays and reads
one value from each. `objectArray` retains 200,000 objects in a managed-reference
array and then reads every element and field. The exact checksum gate runs
before warmups and timing, so a miscompiled or non-equivalent result is excluded
rather than ranked. A compiler may prove the numeric arrays non-escaping and
scalar-replace or stack-place them; that optimization is intentionally part of
this cross-language workload. Use `objectArray` when retained managed-heap work
must remain observable.

Retained-object optimization must not replace the managed graph with a scalar
checksum. The Pop Lang executable must still allocate 200,000 distinct managed
objects, retain their tokens through the managed-reference array, read every
element and field, and print the checksum. Profile-guided allocator work may
amortize page metadata, compact token indexes, inline small payload storage, or
specialize proven barriers. Machine-specific timings are evidence, not a
portable performance promise; record the before/after JSON outside the checked-
in `latest` result unless intentionally publishing a complete benchmark run.
The stable native collector currently stores each logical payload slot as one
physical word and derives slot kind from precise layout metadata; benchmark
changes must retain the scalar-equals-token negative tracing coverage.

The default outputs are `results/latest.json` and `results/latest.html`.
`run` writes JSON only, while `render` can regenerate HTML from an existing
result:

```bash
./bin/benchmark run --samples 9 --output results/run.json
./bin/benchmark render --input results/run.json --output results/run.html
```

Use `list` to inspect IDs. `provision <runtime...>` installs only the portable
toolchains the harness owns: pinned Lua, LuaJIT, and Luau builds plus LDC. Host
Rust, Python, Node.js, Ruby, Go, Clang, .NET, and Crystal installations are
detected and unavailable runtimes are reported and skipped.

## Run under Docker Compose

The Compose service builds the Pop Lang release compiler and portable runtimes,
then runs without network access with two CPUs, 1 GiB of memory, and a 512
process limit. Results are written to this directory's `results/` folder.

```bash
docker compose -f benchmark/compose.yaml build
docker compose -f benchmark/compose.yaml run --rm benchmark
```

When already in `benchmark/`, omit the `benchmark/` prefix. The base image
provides Pop Lang, Rust, Python, Node.js, Ruby, Go, Clang, Lua/LuaJIT/Luau, and
LDC. C# and Crystal participate on a host with those SDKs installed but are
skipped by the base container because their distribution repositories are not
added implicitly.

Container limits improve repeatability, but they do not make results portable
between machines. Record the host CPU, operating system, Docker version, and
whether the host was otherwise idle when publishing comparisons.
