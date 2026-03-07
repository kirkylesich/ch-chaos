---
name: validate-arch
description: Validate that current code matches the arch.md spec. Use when asked to check, validate, or audit the implementation against the specification.
context: fork
agent: Explore
---

Audit the codebase against `docs/arch.md` requirements. Report violations grouped by severity.

## What to check

### 1. Rust conventions
- No `unwrap()`, `expect()`, or `panic!()` in production code (tests are OK)
- Domain values use newtype wrappers (`ExperimentDuration`, `ExperimentId`)
- Errors are typed enums, not strings
- `Phase` and `ScenarioType` are enums

### 2. CRD compliance
- `ChaosExperiment` has all required spec fields: `scenario`, `duration`, `targetNamespace`, `parameters`, `target.edge`
- `ChaosAnalysis` has: `experimentRef`, `prometheus`, `query`, `degradationDirection`, `successCriteria`
- Status fields match spec: `phase`, `message`, `startedAt`, `completedAt`, `experimentId`, `runnerJobs`, `cleanupDone`

### 3. Reconciler logic
- Finalizer `chaos.io/cleanup` added on first reconcile
- Deletion handling: deletionTimestamp → cleanup → remove finalizer
- Phase transitions: Pending → Running → Succeeded/Failed (no skipping)
- Requeue intervals: Running=5s, Succeeded/Failed=300s
- All validation checks from arch.md section 6 are implemented

### 4. Job builder
- `backoffLimit: 0`
- `ttlSecondsAfterFinished: 300`
- `ownerReferences` set to ChaosExperiment
- Prometheus annotations on runner pods
- Env vars: `EXPERIMENT_ID`, `SCENARIO`, `DURATION`, `PARAMETERS`

### 5. Istio injector
- All hosts are FQDN: `{service}.{namespace}.svc.cluster.local`
- Labels: `chaos.io/managed-by: chimp-chaos`, `chaos.io/experiment: {name}`
- `ownerReferences` set
- Default route for non-matching sources included
- `sourceLabels` resolved from Service selector

### 6. GraphBuilder
- Uses `istio_requests_total` metric
- Lookback window configurable (default 10m)
- Edge valid only if `rps >= minRps` (0.05)
- Built on-demand, not cached

### 7. ChaosAnalysis scoring
- Formula matches spec: direction up/down, degradation%, clamp 0-100
- Baseline window: `[startedAt - baselineWindow, startedAt]`
- Chaos window: `[startedAt, completedAt]`

### 8. Metrics
- Operator exposes: `chaos_experiments_total`, `chaos_experiment_duration_seconds`, `chaos_experiment_info`, `chaos_runner_jobs_total`, `chaos_impact_score`
- Runner exposes: `chaos_injection_active`, `chaos_injection_total`, `chaos_targets_affected`, `chaos_injection_duration_seconds`

### 9. RBAC
- Operator SA permissions match spec
- Runner SA has minimal permissions

## Output format

For each finding report:
- **File**: path and line number
- **Severity**: CRITICAL (breaks spec) / WARNING (deviates from spec) / INFO (suggestion)
- **Issue**: what's wrong
- **Expected**: what arch.md says

Summarize with counts: X critical, Y warnings, Z info.
