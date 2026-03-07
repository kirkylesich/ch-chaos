---
name: implement
description: Implement a module or component from the arch.md spec. Use when asked to implement, create, or build a specific part of the chaos operator (e.g. "implement reconciler", "build graph_builder", "create CRD types").
argument-hint: [module-name]
---

Implement the requested module based on the spec in `docs/arch.md`.

## Steps

1. Read `docs/arch.md` fully to understand the spec requirements for the requested module
2. **Fetch up-to-date docs** via context7 MCP for libraries you'll use:
   - `kube-rs/kube` (library ID: `/kube-rs/kube`) — CRD derive, reconciler, finalizers, Controller builder
   - `actix-web` (library ID: `/websites/rs_actix-web_4_11_0`) — HTTP server for /metrics, /health
   - `thiserror` (library ID: `/dtolnay/thiserror`) — error enum derives
   - `tokio` (library ID: `/tokio-rs/tokio`) — async runtime
   - Use `mcp__context7__query-docs` with the appropriate library ID and a specific query for the feature you're implementing
3. Identify which file(s) need to be created or modified based on the project structure in `docs/arch.md` section 12
4. Check existing code to understand what's already implemented and avoid duplication
5. Implement the module following these project rules:

### Rust conventions (mandatory)

- Newtype wrappers for domain values: `ExperimentDuration(NonZeroU64)`, `ExperimentId(Uuid)`
- Typed error enums (`ValidationError`, `RunnerError`) — never use string errors
- `Phase` and `ScenarioType` must be enums
- **Zero** `unwrap()`, `expect()`, or `panic!()` in production code
- All errors via `Result`/`Option` with `?` propagation
- Use `thiserror` for error derives

### Validation rules (for reconciler/CRD code)

- `duration > 0` otherwise Failed: `"duration must be greater than 0"`
- scenario must be known enum variant, otherwise Failed: `"unknown scenario"`
- `targetNamespace` defaults to experiment's namespace if omitted
- Pod/Node chaos: must find target nodes, otherwise Failed: `"no target nodes found"`
- Edge chaos: `sourceService` and `destinationService` required
- Edge chaos: resolve source Service selector, check VirtualService conflicts, verify edge via GraphBuilder

### Key constants

- Reconcile requeue Running: 5 seconds
- Reconcile requeue Succeeded/Failed: 300 seconds
- Job `backoffLimit: 0`, `ttlSecondsAfterFinished: 300`
- GraphBuilder `minRps: 0.05`, `graphLookback: 10m`
- All Istio VirtualService hosts use FQDN: `{service}.{namespace}.svc.cluster.local`

6. Add the module to the appropriate `mod.rs`
7. **Write tests with mocks** for the implemented module:
   - Define traits for external dependencies (K8s API, Prometheus, Istio)
   - Use `mockall` (`#[automock]`) to generate mock implementations
   - Use `wiremock` for HTTP API mocks (Prometheus queries)
   - Use `tower-test` for kube client mocks where applicable
   - Cover: happy path, validation errors, edge cases, failure paths
   - If unsure how to mock something, use `WebSearch` or context7 to find patterns
8. Ensure `cargo test` passes

## Module reference

Implement `$ARGUMENTS` following the structure:

```
src/
├── main.rs                  # CLI: --mode operator | runner
├── operator/
│   ├── crd.rs               # ChaosExperiment, ChaosAnalysis CRD types
│   ├── reconciler.rs        # Reconcile logic
│   ├── job_builder.rs       # RunnerJobInjector — Job specs
│   ├── istio_injector.rs    # IstioEdgeInjector — VirtualService CRUD
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
