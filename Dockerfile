# Dockerfile for the BSR Remote Plugin `buf.build/nu-sync/rust-temporal`.
#
# BSR Remote Plugins run a single binary against the stdin/stdout
# CodeGeneratorRequest / CodeGeneratorResponse contract. Build a static
# release binary and copy it into a `distroless` runtime — no shell, no libc,
# minimal attack surface, ~5 MB final image.

FROM --platform=$BUILDPLATFORM rust:1.85-slim AS build
WORKDIR /src

RUN apt-get update \
    && apt-get install -y --no-install-recommends protobuf-compiler ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# Cache deps separately from source.
COPY Cargo.toml rust-toolchain.toml ./
COPY crates/protoc-gen-rust-temporal/Cargo.toml crates/protoc-gen-rust-temporal/
COPY crates/temporal-proto-runtime/Cargo.toml crates/temporal-proto-runtime/
RUN mkdir -p crates/protoc-gen-rust-temporal/src crates/temporal-proto-runtime/src \
    && echo 'fn main() {}' > crates/protoc-gen-rust-temporal/src/main.rs \
    && echo '' > crates/protoc-gen-rust-temporal/src/lib.rs \
    && echo '' > crates/temporal-proto-runtime/src/lib.rs

# Pre-warm the dep cache. `--locked` is intentionally omitted (Cargo.lock is
# gitignored) so the build picks up the latest minor versions of deps.
RUN cargo build --release --bin protoc-gen-rust-temporal || true

COPY crates ./crates
RUN cargo build --release --bin protoc-gen-rust-temporal

FROM gcr.io/distroless/cc-debian12:nonroot
COPY --from=build /src/target/release/protoc-gen-rust-temporal /usr/local/bin/protoc-gen-rust-temporal
ENTRYPOINT ["/usr/local/bin/protoc-gen-rust-temporal"]
