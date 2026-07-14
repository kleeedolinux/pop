# eBPF XDP Example

This directory contains the initial experimental eBPF example for Pop Lang.

Build the minimal XDP program:

```sh
pop build examples/bpf/xdpPass.pop \
    --target bpfel-unknown-none \
    --runtime-profile linux-ebpf \
    --bpf-program xdp \
    --emit-object target/xdp-pass.o
```

The program returns numeric `2`, the Linux `XDP_PASS` action. The emitted
object is an ELF eBPF object with an `xdp` section and a `pop_bpf_xdp` entry
wrapper.

Inspect the object with ordinary ELF tools when the installed LLVM supports the
BPF target:

```sh
file target/xdp-pass.o
readelf -h target/xdp-pass.o
readelf -S target/xdp-pass.o
llvm-objdump -h target/xdp-pass.o
llvm-objdump -d target/xdp-pass.o
```

The selected `linux-ebpf` runtime profile satisfies only the contracts needed
by this scalar example. It does not attach the program to an interface, access
packet bytes, define maps, emit BTF, support CO-RE, or use helpers/ring
buffers. Loading and attaching eBPF programs may require a compatible Linux
kernel and privileges.
