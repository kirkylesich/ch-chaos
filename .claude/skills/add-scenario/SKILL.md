---
name: add-scenario
description: Add a new chaos scenario to the operator. Use when asked to add, create, or implement a new chaos scenario type.
argument-hint: [scenario-name]
disable-model-invocation: true
---

Add a new chaos scenario `$ARGUMENTS` to the operator.

## Steps

1. Read `docs/arch.md` to understand existing scenario patterns
2. Read existing scenarios in `src/runner/scenarios/` for reference
3. Determine the scenario class:
   - **Pod/Node chaos** → uses RunnerJobInjector, creates runner Jobs
   - **Edge chaos** → uses IstioEdgeInjector, creates Istio VirtualService

4. For Pod/Node scenario, create:
   - `src/runner/scenarios/{scenario_name}.rs` — scenario implementation
   - Add variant to `ScenarioType` enum in `src/operator/types.rs`
   - Register in `src/runner/scenarios/mod.rs`
   - Add parameters struct if needed
   - Add security context requirements (privileged, capabilities)

5. For Edge scenario, create:
   - Add variant to `ScenarioType` enum
   - Add fault policy generation in `src/operator/istio_injector.rs`
   - Add parameters (what fields the user specifies in CRD)

6. Update validation in reconciler for the new scenario
7. Create example YAML in `examples/{scenario-name}.yaml`
8. Ensure `cargo check` passes

## Conventions

- Scenario name in CRD: PascalCase (e.g., `CpuStress`, `NetworkDelay`)
- File name: snake_case (e.g., `cpu_stress.rs`, `network_delay.rs`)
- No `unwrap()` / `expect()` / `panic!()` — use typed errors
- Runner scenarios must handle graceful shutdown on SIGTERM
