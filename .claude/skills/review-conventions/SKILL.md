---
name: review-conventions
description: Review code for project conventions and Rust best practices specific to this codebase. Use when reviewing a PR, checking code quality, or when asked to review.
user-invocable: false
---

When reviewing code in this project, check for these project-specific conventions:

## Hard rules (must fix)

- No `unwrap()`, `expect()`, or `panic!()` in production code (tests are OK)
- All domain values wrapped in newtypes (`ExperimentDuration(NonZeroU64)`, `ExperimentId(Uuid)`)
- Errors are typed enums with `thiserror`, never `String` or `anyhow` in library code
- `Phase` transitions must follow: Pending → Running → Succeeded/Failed (no skipping)
- Finalizer `chaos.io/cleanup` must be added before any work in reconcile
- All Istio VirtualService hosts must be FQDN format
- `sourceLabels` must come from resolved Service selector, never hardcoded
- Jobs must have `backoffLimit: 0` and `ownerReferences`

## Patterns to prefer

- `?` operator for error propagation, not `match` on every Result
- `kube::Api` for K8s operations
- Environment variables for configuration with documented defaults
- Prometheus metrics registered at startup, not lazily

## Anti-patterns to flag

- Using Prometheus to monitor Job lifecycle (use K8s API instead)
- Caching the service graph (must be built on-demand)
- Creating runner pods for edge chaos (edge chaos uses Istio only)
- String-based phase comparisons (use Phase enum)
- Missing default route in VirtualService (non-matching sources must still route)
