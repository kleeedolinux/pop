# Benchmarks

This directory compares equivalent Fibonacci, integer-loop, and array-loop
workloads across Lua 5.5.0, LuaJIT, Luau 0.729, Luau native code generation,
and Pop Lang's release `pop run` path. Compilation and tool provisioning are
outside the timed region.

The runner requires Ruby 3.2 or newer, plus `curl`, `tar`, `unzip`, `make`, and
a C compiler for the source-built Lua and LuaJIT runtimes.

```bash
ruby benchmark/bin/benchmark provision
ruby benchmark/bin/benchmark run --samples 9
ruby benchmark/bin/benchmark render --input benchmark/results/latest.json
```

The portable interpreters live in `benchmark/.tools/` (ignored by Git). Lua is
built from the requested source archive; LuaJIT is built locally; Luau is taken
from its official 0.729 release archive. `luaujit` invokes Luau's optional
`--codegen` mode and stops clearly if that release does not provide it.

Use `list` to inspect the benchmark matrix, `provision <runtime...>` to fetch
only selected runtimes, and repeat `--runtime`/`--workload` to narrow a run.
The generated HTML is self-contained: each result is a ball whose crossing
speed is proportional to its median measured execution time.
