FROM rust:1.89-bookworm AS builder

WORKDIR /app

# 1. Cache dependencies: copy manifests, create dummy sources, build deps only
COPY Cargo.toml Cargo.lock* ./
RUN mkdir -p src/bin src/operator src/runner/scenarios && \
    echo "fn main() {}" > src/main.rs && \
    echo "pub mod operator; pub mod runner;" > src/lib.rs && \
    echo "pub mod crd; pub mod graph_builder; pub mod istio_injector; pub mod job_builder; pub mod reconciler; pub mod types; pub mod controller; pub mod kube_client;" > src/operator/mod.rs && \
    echo "pub mod entry; pub mod metrics; pub mod scenarios; pub mod server;" > src/runner/mod.rs && \
    touch src/operator/crd.rs src/operator/graph_builder.rs src/operator/istio_injector.rs \
          src/operator/job_builder.rs src/operator/reconciler.rs src/operator/types.rs \
          src/operator/controller.rs src/operator/kube_client.rs \
          src/runner/entry.rs src/runner/metrics.rs src/runner/server.rs && \
    echo "pub mod cpu_stress; pub mod network_delay; pub mod pod_killer;" > src/runner/scenarios/mod.rs && \
    touch src/runner/scenarios/cpu_stress.rs src/runner/scenarios/network_delay.rs \
          src/runner/scenarios/pod_killer.rs && \
    echo "fn main() {}" > src/bin/crd_gen.rs

RUN cargo build --release 2>/dev/null || true
RUN rm -rf src/ target/release/chimp-chaos target/release/crd_gen target/release/deps/chimp_chaos* target/release/.fingerprint/chimp-chaos-*

# 2. Copy real source and build
COPY src/ src/
RUN cargo build --release --bin chimp-chaos

FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    stress-ng \
    iproute2 \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/chimp-chaos /usr/local/bin/chimp-chaos

ENTRYPOINT ["chimp-chaos"]
