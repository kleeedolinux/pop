FROM ubuntu:26.04

ARG DEBIAN_FRONTEND=noninteractive

RUN apt-get update \
    && apt-get install --yes --no-install-recommends \
        build-essential ca-certificates clang-22 curl golang-go libpolly-22-dev \
        llvm-22-dev nodejs python3 ruby-full rustc cargo tar unzip xz-utils \
    && rm -rf /var/lib/apt/lists/*

ENV LLVM_SYS_221_PREFIX=/usr/lib/llvm-22
WORKDIR /workspace

COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY assets ./assets
COPY benchmark ./benchmark

RUN cargo build --release --package pop-driver --bin pop \
    && ruby benchmark/bin/benchmark provision lua luajit luau luaujit d

ENTRYPOINT ["benchmark/bin/benchmark"]
CMD ["batch"]
