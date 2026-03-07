# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

**Chimp Chaos Operator** — a Kubernetes operator for chaos engineering, written in Rust. Single binary, two modes:

- **Operator mode** (`chimp-chaos --mode operator`): Deployment that watches ChaosExperiment/ChaosAnalysis CRDs, spawns runner Jobs or applies Istio fault policies
- **Runner mode** (`chimp-chaos --mode runner`): Job pod on a target node that executes chaos injection and exposes Prometheus metrics on `:9090/metrics`

Full spec is in `docs/arch.md`.

## Build & Run

```bash
cargo build
cargo run -- --mode operator
cargo run -- --mode runner
cargo test
cargo test <test_name>          # run a single test
cargo clippy                    # lint
cargo fmt                       # format
```

## Architecture

### Two chaos classes with different injection mechanisms

| Class | Scenarios | Injector | Creates |
|-------|-----------|----------|---------|
| Pod/Node Chaos | PodKiller, CpuStress, NetworkDelay | RunnerJobInjector | K8s Job with `--mode runner` |
| Single-Hop Edge Chaos | EdgeDelay, EdgeAbort | IstioEdgeInjector | Istio VirtualService fault policy |

### Key CRDs

- **ChaosExperiment** (`chaos.io/v1`): defines a chaos scenario with target, duration, and parameters. Status tracks phase (Pending → Running → Succeeded/Failed), runner job names, and cleanup state.
- **ChaosAnalysis** (`chaos.io/v1`): post-experiment impact scoring. References a completed experiment, queries Prometheus for baseline vs chaos windows, computes impact score 0-100, and renders Pass/Fail verdict.

### Experiment lifecycle

- Operator adds `chaos.io/cleanup` finalizer on first reconcile
- **Pod/Node chaos**: operator creates Jobs → monitors Job status via K8s API (NOT Prometheus) → updates phase on completion → cleans up Jobs
- **Edge chaos**: operator resolves source workload labels from K8s Service selector → checks for conflicting VirtualService → builds observed graph via Prometheus PromQL → creates Istio VirtualService fault policy → waits for duration → deletes policy
- Cleanup guaranteed via finalizers + ownerReferences (belt and suspenders)

### Observed Service Graph (GraphBuilder)

Built on-demand (not cached) from Istio telemetry (`istio_requests_total`) via Prometheus. Used only for edge chaos pre-flight validation: confirms target edge exists and has traffic above `minRps` threshold.

### Project structure

```
src/
├── main.rs                  # CLI: --mode operator | runner
├── operator/
│   ├── crd.rs               # ChaosExperiment, ChaosAnalysis CRD types
│   ├── reconciler.rs        # Reconcile logic
│   ├── job_builder.rs       # RunnerJobInjector — construct Job specs
│   ├── istio_injector.rs    # IstioEdgeInjector — Istio VirtualService CRUD
│   ├── graph_builder.rs     # On-demand observed graph from Prometheus
│   └── types.rs             # Newtype wrappers, typed errors
└── runner/
    ├── server.rs            # HTTP server: /metrics, /health
    ├── metrics.rs           # Prometheus metrics registry
    └── scenarios/
        ├── pod_killer.rs    # K8s API pod delete
        ├── cpu_stress.rs    # stress-ng wrapper
        └── network_delay.rs # tc netem wrapper
```

## Rust Conventions

- Newtype wrappers for domain values: `ExperimentDuration(NonZeroU64)`, `ExperimentId(Uuid)`
- Typed error enums (`ValidationError`, `RunnerError`) — no string errors
- Phase and ScenarioType are enums
- No `unwrap()`, `expect()`, or `panic!()` in production code — all errors via `Result`/`Option` with `?` propagation
- No `unsafe` blocks unless absolutely necessary (and must be justified with a comment)
- All public API types must derive `Clone`, `Debug`; CRD types also `Serialize`, `Deserialize`, `JsonSchema`
- Validate all external input at system boundaries (CRD spec fields, env vars, Prometheus responses)
- Use `NonZeroU64` for duration, `Uuid` for experiment IDs — invalid states must be unrepresentable at the type level
- Exhaustive `match` on enums — no wildcard `_` catch-all for `Phase` and `ScenarioType` (compiler catches new variants)

## Key Design Decisions

- Operator monitors runner Jobs via **K8s API**, not Prometheus. Prometheus is only used for: (1) pre-flight edge resolution via GraphBuilder, (2) post-experiment impact analysis via ChaosAnalysis
- Edge chaos does NOT create runner pods — chaos is applied through Istio data plane
- All Istio VirtualService hosts use FQDN (`{service}.{namespace}.svc.cluster.local`)
- MVP: edge chaos fails if a conflicting (non-chaos-managed) VirtualService exists for the destination host — no patch/merge
- Source workload labels resolved from `Service.spec.selector`, never assumed

## Dependencies

| Crate | Purpose | context7 library ID |
|-------|---------|---------------------|
| `kube` (with `runtime`, `derive`, `client`) | K8s operator framework: CRD derive, Controller, reconciler, finalizers | `/kube-rs/kube` |
| `k8s-openapi` (feature `latest`) | K8s API types (Job, Pod, Service, etc.) | — |
| `schemars` | JSON Schema generation for CRD | — |
| `tokio` (full) | Async runtime | `/tokio-rs/tokio` |
| `futures` | Stream combinators for controller | — |
| `actix-web` | HTTP server for `/metrics` and `/health` endpoints | `/websites/rs_actix-web_4_11_0` |
| `serde` + `serde_json` + `serde_yaml` | Serialization | — |
| `thiserror` | Typed error enum derives | `/dtolnay/thiserror` |
| `anyhow` | Top-level error handling (main.rs only) | — |
| `tracing` + `tracing-subscriber` | Structured logging | — |
| `prometheus` | Metrics registry and exposition | — |
| `reqwest` (with `json`) | HTTP client for Prometheus queries (GraphBuilder) | — |
| `clap` (with `derive`) | CLI argument parsing (`--mode operator\|runner`) | — |
| `uuid` (with `v4`, `serde`) | `ExperimentId` newtype | — |
| `chrono` (with `serde`) | Timestamps in CRD status | — |

When implementing:
- Always fetch up-to-date docs via context7 MCP (`mcp__context7__query-docs`) with the library ID from this table before using any API
- Use WebSearch/WebFetch for patterns not covered by context7
- Always use the **latest stable versions** of all crates — verify via context7 or WebSearch before adding to Cargo.toml

## Testing

All code must be covered with tests using mocks. No real K8s cluster or Prometheus required for tests.

### Approach

- Use **trait-based dependency injection** for all external dependencies (K8s API, Prometheus, Istio)
- Define traits (`KubeClient`, `PrometheusClient`, `IstioClient`) and mock them in tests
- Use `mockall` crate for generating mock implementations
- Use `kube::Client` test utilities (`kube::client::Body`, tower-test) where applicable

### What to test

| Module | What to mock | Key test cases |
|--------|-------------|----------------|
| `reconciler` | K8s API (Job CRUD, status patch) | Phase transitions, validation errors, finalizer add/remove, cleanup on deletion |
| `job_builder` | — (pure function) | Job spec correctness: backoffLimit, ttl, ownerRefs, env vars, annotations |
| `istio_injector` | K8s API (VirtualService CRUD, Service get) | FQDN hosts, sourceLabels from selector, conflict detection, default route |
| `graph_builder` | Prometheus HTTP API | Edge found/not found, rps threshold, malformed response |
| `ChaosAnalysis` | Prometheus HTTP API, K8s API | Scoring formula (direction up/down), clamp 0-100, verdict Pass/Fail, missing experiment |
| `scenarios/*` | K8s API (pod delete), system commands (stress-ng, tc) | Success/failure paths, graceful shutdown |
| `server` | — (integration) | `/metrics` returns prometheus format, `/health` returns 200 |

### Test dependencies

| Crate | Purpose |
|-------|---------|
| `mockall` | Mock trait generation (`#[automock]`) |
| `tokio::test` | Async test runtime |
| `tower-test` | Mock K8s API server for kube client tests |
| `wiremock` | Mock HTTP server for Prometheus API tests |
