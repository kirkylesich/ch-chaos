# Chimp Chaos Operator — Полное ТЗ

## 1. Что это

Kubernetes оператор для chaos engineering. Один бинарник, два режима:

- **Operator mode** (`chimp-chaos --mode operator`) — Deployment, управляет экспериментами через CRD, спавнит runner Jobs, применяет Istio fault policies
- **Runner mode** (`chimp-chaos --mode runner`) — запускается как Job pod на целевой ноде, выполняет chaos инъекцию, экспоузит метрики

```
┌──────────────────────────────────────────────────────────────┐
│                    Kubernetes Cluster                        │
│                                                              │
│  ┌────────────┐         ┌───────────┐                         │
│  │  Operator  │────────▶│  K8s API  │                         │
│  │ (Deployment)│◀───────│           │                         │
│  │ --mode     │         └───────────┘                         │
│  │  operator  │                                                │
│  └─────┬──────┘                                                │
│        │ creates Jobs                                          │
│        ▼                                                       │
│  ┌───────────┐  ┌───────────┐  ┌───────────┐                 │
│  │ Runner Job│  │ Runner Job│  │ Runner Job│                 │
│  │ (Node 1)  │  │ (Node 2)  │  │ (Node 3)  │                 │
│  │ --mode    │  │ --mode    │  │ --mode    │                 │
│  │  runner   │  │  runner   │  │  runner   │                 │
│  │ :9090     │  │ :9090     │  │ :9090     │                 │
│  │ /metrics  │  │ /metrics  │  │ /metrics  │                 │
│  └───────────┘  └───────────┘  └───────────┘                 │
│        │              │              │                         │
│        └──────────────┼──────────────┘                        │
│                       ▼                                        │
│                ┌────────────┐       ┌────────────┐            │
│                │ Prometheus │◀──────│ App metrics│            │
│                │            │       │ (SLIs)     │            │
│                └─────┬──────┘       └────────────┘            │
│                      ▼                                         │
│               ┌────────────────┐                              │
│               │ ChaosAnalysis  │ ← impact score 0-100          │
│               │ (CRD)          │                              │
│               └────────────────┘                              │
└──────────────────────────────────────────────────────────────┘
```

### Два класса chaos

| Класс | Сценарии | Injector | Что создаётся |
|-------|----------|----------|---------------|
| Pod / Node Chaos | PodKiller, CpuStress, NetworkDelay | RunnerJobInjector | Kubernetes Job с `--mode runner` |
| Single-Hop Edge Chaos (MVP) | EdgeDelay, EdgeAbort | IstioEdgeInjector | Istio VirtualService fault policy |

Edge chaos в MVP — это chaos на одном observed edge между двумя сервисами.
Это НЕ path-level chaos, НЕ multi-hop flow, НЕ trace-based reconstruction.
Поддерживается только: `sourceService → destinationService` (один hop).

## 2. Один бинарник, два режима

Один Docker image `chimp-chaos:latest`, один Cargo binary. Режим определяется CLI аргументом.

```
chimp-chaos --mode operator    # Deployment: watch CRD, create Jobs, manage lifecycle
chimp-chaos --mode runner      # Job pod: execute chaos, expose /metrics, exit
```

### Operator mode

- Запускается как Deployment (1 реплика)
- Watch-ит ChaosExperiment CRD
- При Pending → создаёт Job(s) с `--mode runner` на целевых нодах
- Мониторит Job status через K8s API (не Prometheus)
- При завершении Job → обновляет phase эксперимента
- Экспоузит operator-level метрики на `/metrics`

### Runner mode

- Запускается как Job pod оператором
- Получает параметры через env переменные
- Стартует HTTP сервер на `:9090` с `/metrics`
- Выполняет chaos сценарий (stress-ng, tc, K8s API delete)
- Экспоузит runner-level метрики (Prometheus scrape)
- По завершении duration — exit 0 (success) или exit 1 (failure)

### Job spec (создаётся оператором)

```yaml
apiVersion: batch/v1
kind: Job
metadata:
  name: chaos-runner-{experiment-id}-{node}
  namespace: default
  labels:
    app: chimp-chaos-runner
    chaos.io/experiment: pod-killer-test
    chaos.io/scenario: CpuStress
  ownerReferences:             # GC: Job удаляется вместе с ChaosExperiment
    - apiVersion: chaos.io/v1
      kind: ChaosExperiment
      name: pod-killer-test
spec:
  backoffLimit: 0              # не перезапускать при failure
  ttlSecondsAfterFinished: 300 # автоочистка через 5 мин
  template:
    metadata:
      labels:
        app: chimp-chaos-runner
      annotations:
        prometheus.io/scrape: "true"
        prometheus.io/port: "9090"
    spec:
      containers:
        - name: chaos-runner
          image: chimp-chaos:latest
          args: ["--mode", "runner"]
          env:
            - name: EXPERIMENT_ID
              value: "550e8400-e29b-41d4-a716-446655440000"
            - name: SCENARIO
              value: "CpuStress"
            - name: DURATION
              value: "300"
            - name: PARAMETERS
              value: '{"cores":2,"percent":80}'
          ports:
            - containerPort: 9090
              name: metrics
          securityContext:
            privileged: true
      nodeName: target-node-01
      restartPolicy: Never
      serviceAccountName: chimp-chaos-runner
```

## 3. CRD: ChaosExperiment

### Pod / Node target

```yaml
apiVersion: chaos.io/v1
kind: ChaosExperiment
metadata:
  name: pod-killer-test
  namespace: default
spec:
  scenario: PodKiller          # PodKiller | CpuStress | NetworkDelay
  duration: 300                # секунды, > 0
  targetNamespace: production  # optional, default = namespace эксперимента
  parameters:                  # optional, scenario-specific
    gracePeriod: 30
```

### Edge target (Single-Hop Edge Chaos)

```yaml
apiVersion: chaos.io/v1
kind: ChaosExperiment
metadata:
  name: payment-ledger-delay
  namespace: default
spec:
  scenario: EdgeDelay          # EdgeDelay | EdgeAbort
  duration: 600
  target:
    namespace: production
    edge:
      sourceService: payment
      destinationService: ledger
  parameters:
    latencyMs: 200             # EdgeDelay: задержка в мс
    # abortPercent: 50         # EdgeAbort: % запросов с ошибкой
    # abortHttpStatus: 503     # EdgeAbort: HTTP код ошибки
```

### Status (заполняется кубер оператором)

```yaml
status:
  phase: Running               # Pending → Running → Succeeded/Failed
  message: "Running on 3 nodes for 300s"
  startedAt: "2025-03-07T10:00:00Z"
  completedAt: null
  experimentId: "550e8400-e29b-41d4-a716-446655440000"
  runnerJobs:                  # Jobs которые оператор создал
    - "chaos-runner-550e8400-node01"
    - "chaos-runner-550e8400-node02"
  cleanupDone: false
```

## 4. Жизненный цикл эксперимента

### Pod / Node Chaos flow

```
kubectl apply -f experiment.yaml
        │
        ▼
  K8s API Server (создаёт CR)
        │
        ▼ watch event
  Operator: reconcile()
        │
        ├─ 1. Добавить finalizer chaos.io/cleanup (если нет)
        │
        ├─ 2. Проверить deletionTimestamp
        │     └─ если есть → удалить Jobs → убрать finalizer → CR удаляется
        │
        ├─ 3. Phase: Pending
        │     ├─ validate (duration > 0, scenario known)
        │     ├─ выбрать injector по scenario type
        │     ├─ [RunnerJobInjector] определить целевые ноды → создать Jobs
        │     ├─ сохранить имена Jobs в status.runnerJobs
        │     └─ phase → Running
        │
        ├─ 4. Phase: Running (requeue каждые 5 сек)
        │     ├─ проверить Job status через K8s API (НЕ Prometheus)
        │     ├─ если все Jobs succeeded или duration истёк → phase → Succeeded
        │     ├─ если любой Job failed → phase → Failed
        │     └─ иначе requeue
        │
        └─ 5. Phase: Succeeded/Failed
              ├─ если !cleanupDone: удалить Jobs через K8s API
              ├─ cleanupDone = true
              └─ requeue каждые 300 сек (idle)
```

### Single-Hop Edge Chaos flow

```
kubectl apply -f edge-experiment.yaml
        │
        ▼
  Operator: reconcile()
        │
        ├─ 1. Добавить finalizer
        │
        ├─ 2. Phase: Pending
        │     ├─ validate spec (scenario, duration, edge target)
        │     ├─ resolve source workload labels (K8s Service → spec.selector)
        │     │   └─ если Service не найден → phase → Failed
        │     ├─ проверить нет ли конфликтующего VirtualService для destination host
        │     │   └─ если есть (без label chaos.io/managed-by) → phase → Failed
        │     ├─ GraphBuilder: build observed graph (on-demand)
        │     │   └─ PromQL query к Prometheus за последние 10 мин
        │     ├─ проверить что target edge существует
        │     │   └─ если edge не найден → phase → Failed
        │     ├─ [IstioEdgeInjector] создать Istio VirtualService fault policy
        │     │   └─ hosts: FQDN, sourceLabels: из resolved selector
        │     └─ phase → Running
        │
        ├─ 3. Phase: Running (requeue каждые 5 сек)
        │     ├─ проверить duration elapsed
        │     └─ если duration истёк → phase → Succeeded
        │
        └─ 4. Phase: Succeeded/Failed
              ├─ если !cleanupDone: удалить Istio fault policy
              ├─ cleanupDone = true
              └─ requeue каждые 300 сек
```

Ключевое отличие: edge chaos НЕ создаёт runner Jobs и runner pods.
Chaos выполняется через Istio dataplane.
Все hosts в VirtualService используют FQDN для предотвращения namespace misresolution.

### Два уровня мониторинга

| Уровень | Источник | Для чего |
|---------|----------|----------|
| Lifecycle (control plane) | K8s Job status | Оператор знает КОГДА эксперимент начался/закончился |
| Pre-flight edge resolution | Prometheus (GraphBuilder) | Оператор проверяет существование target edge перед запуском edge chaos |
| Scoring (data plane) | Prometheus | ChaosAnalysis знает КАК система себя вела |

Оператор НЕ использует Prometheus для отслеживания выполнения runner Jobs.
Prometheus используется для:
- **Pre-flight**: observed edge resolution через GraphBuilder (только для edge chaos, до старта)
- **Post-factum**: анализ impact через ChaosAnalysis (после завершения любого эксперимента)

### Гарантия cleanup

Kubernetes Finalizer `chaos.io/cleanup`:
- Добавляется при первом reconcile
- При `kubectl delete` K8s ставит deletionTimestamp, но НЕ удаляет CR
- Оператор видит deletionTimestamp → удаляет Jobs → убирает finalizer
- Только после этого CR реально удаляется

Дополнительно: `ownerReferences` на Jobs ссылаются на ChaosExperiment.
Если CR удалён — K8s GC автоматически удалит Jobs (belt and suspenders).

## 5. Сценарии и Injector Types

### Injector Types

Оператор выбирает механизм инъекции в зависимости от типа сценария.

| Сценарий | Injector | Что создаётся |
|----------|----------|---------------|
| PodKiller | RunnerJobInjector | K8s Job → runner pod |
| CpuStress | RunnerJobInjector | K8s Job → runner pod |
| NetworkDelay | RunnerJobInjector | K8s Job → runner pod |
| EdgeDelay | IstioEdgeInjector | Istio VirtualService fault policy |
| EdgeAbort | IstioEdgeInjector | Istio VirtualService fault policy |

### RunnerJobInjector (Pod / Node Chaos)

Поведение без изменений:
```
Operator → создаёт Kubernetes Job → Job запускает chimp-chaos --mode runner → runner выполняет chaos
```

### IstioEdgeInjector (Single-Hop Edge Chaos)

- Runner pods НЕ создаются
- Оператор применяет временную Istio VirtualService fault policy
- Policy действует только между указанными сервисами (source → destination)
- По окончании эксперимента policy удаляется

#### VirtualService ownership policy (MVP)

MVP работает только с destination host'ами, для которых НЕТ существующего VirtualService.
Если для destination host уже существует VirtualService (созданный не chaos-оператором),
эксперимент переходит в Failed с сообщением:
`"conflicting VirtualService exists for host: {host}"`.

Это ограничение MVP. В будущем возможен patch/merge существующих VirtualService,
но это требует аккуратной работы с Istio route merging caveats.

#### Source workload label resolution

`sourceService` в CRD — это логическое имя сервиса. Для Istio `sourceLabels` нужны
реальные workload labels пода. Оператор резолвит их так:

1. Найти Kubernetes Service по имени `sourceService` в target namespace
2. Взять `spec.selector` этого Service — это и есть workload labels
3. Использовать эти labels в `sourceLabels` VirtualService match

Если Service не найден или у него нет selector — эксперимент Failed:
`"cannot resolve workload labels for source service: {name}"`.

#### Пример создаваемой Istio policy

```yaml
apiVersion: networking.istio.io/v1beta1
kind: VirtualService
metadata:
  name: chaos-edge-{experiment-id}
  namespace: production
  labels:
    chaos.io/experiment: payment-ledger-delay
    chaos.io/managed-by: chimp-chaos    # marker for ownership check
  ownerReferences:
    - apiVersion: chaos.io/v1
      kind: ChaosExperiment
      name: payment-ledger-delay
spec:
  hosts:
    - ledger.production.svc.cluster.local   # FQDN to avoid namespace misresolution
  http:
    - match:
        - sourceLabels:
            app: payment         # resolved from Service.spec.selector, NOT assumed
      fault:
        delay:
          percentage:
            value: 100
          fixedDelay: 200ms
      route:
        - destination:
            host: ledger.production.svc.cluster.local
    - route:                     # default route (no fault) for other sources
        - destination:
            host: ledger.production.svc.cluster.local
```

Все hosts используют FQDN (`{service}.{namespace}.svc.cluster.local`)
для предотвращения namespace misresolution, как рекомендует Istio.

### Pod / Node сценарии

| Сценарий | Что делает runner | Privileged | Capabilities |
|----------|-------------------|-----------|-------------|
| PodKiller | K8s API: delete pods по selector | Нет | — |
| CpuStress | Запускает stress-ng внутри пода | Да | — |
| NetworkDelay | tc qdisc на сетевом интерфейсе ноды | Да | NET_ADMIN |

### Edge сценарии

| Сценарий | Что делает | Параметры |
|----------|-----------|-----------|
| EdgeDelay | Istio fault delay между source → destination | latencyMs |
| EdgeAbort | Istio fault abort между source → destination | abortPercent, abortHttpStatus |

## 6. Валидация

При переходе Pending → Running оператор проверяет:

| Проверка | Результат при ошибке |
|----------|---------------------|
| duration == 0 | Failed: "duration must be greater than 0" |
| scenario неизвестен | Failed: "unknown scenario" |
| targetNamespace не указан | Используется namespace эксперимента |
| нет нод для target pods (pod/node chaos) | Failed: "no target nodes found" |
| edge target без sourceService или destinationService | Failed: "edge target requires sourceService and destinationService" |
| edge не найден в observed graph | Failed: "target edge not found: payment → ledger" |
| edge traffic ниже minRps (0.05) | Failed: "edge traffic below threshold" |
| sourceService не резолвится в workload labels | Failed: "cannot resolve workload labels for source service: {name}" |
| существует конфликтующий VirtualService для destination host | Failed: "conflicting VirtualService exists for host: {host}" |

## 7. Observed Service Graph (GraphBuilder)

### Концепция

Для edge chaos оператор должен проверить что целевая зависимость реально существует.
Для этого используется observed graph, построенный из Istio telemetry через Prometheus.

Граф НЕ хранится постоянно и НЕ обновляется в фоне.
Он строится on-demand при запуске каждого edge эксперимента.

### Источник данных

Стандартные Istio метрики в Prometheus:

```
istio_requests_total
istio_request_duration_milliseconds
```

Ключевые labels:
- `source_workload`, `source_workload_namespace`
- `destination_workload`, `destination_workload_namespace`
- `destination_service_name`

### PromQL queries для GraphBuilder

```promql
# Все edges за последние 10 минут с RPS
sum by (
  source_workload,
  source_workload_namespace,
  destination_workload,
  destination_workload_namespace,
  destination_service_name
) (
  rate(istio_requests_total[10m])
)
```

Результат — список edges с RPS:
```
payment (production) → ledger (production) : 12.5 rps
frontend (production) → api (production) : 450.2 rps
api (production) → payment (production) : 45.0 rps
```

### Graph Lookback Window

| Параметр | Default | Описание |
|----------|---------|----------|
| graphLookback | 10m | Окно для построения графа |
| minRps | 0.05 | Минимальный traffic для валидного edge |

Edge считается валидным только если `rps >= minRps`.
Если traffic ниже порога — эксперимент не запускается.

### GraphBuilder flow

```
Operator reconcile (edge chaos, Pending)
        │
        ▼
  1. Resolve source workload labels
  │   └─ K8s API: get Service → spec.selector
  │   └─ если не найден → Err(SourceServiceNotFound)
        │
        ▼
  2. Check VirtualService conflict
  │   └─ K8s API: list VirtualService for destination FQDN
  │   └─ если есть без label chaos.io/managed-by → Err(ConflictingVirtualService)
        │
        ▼
  GraphBuilder.resolve_edge(source, destination, namespace)
        │
        ├─ 3. PromQL query к Prometheus (rate за graphLookback)
        │
        ├─ 4. Найти edge source → destination в результатах
        │
        ├─ 5. Проверить rps >= minRps
        │
        ├─ если edge найден и traffic достаточный → Ok(EdgeInfo)
        │
        └─ если edge не найден или traffic < minRps → Err(EdgeNotFound)
```

GraphBuilder — модуль оператора. Не отдельный сервис, не отдельный pod.

### Ограничения MVP

Edge chaos в MVP поддерживает только:
- single-hop service-to-service edge (один observed edge между двумя сервисами)
- destination host'ы без существующих конфликтующих VirtualService
- source service с резолвимым Kubernetes Service и непустым spec.selector

НЕ поддерживается:
- multi-hop flows (frontend → api → payment → ledger как цепочка)
- operation-level targeting (конкретный HTTP path/method)
- trace-based path reconstruction
- automatic flow discovery
- patch/merge существующих VirtualService (конфликт = отказ)
- source workload без Kubernetes Service (напр. CronJob без headless Service)

## 8. Impact Score и ChaosAnalysis

### Концепция

После завершения эксперимента пользователь создаёт `ChaosAnalysis` — отдельный CRD который:
1. Берёт timestamps из завершённого эксперимента
2. Запрашивает Prometheus за два периода (baseline и chaos)
3. Считает impact score 0-100 (насколько сильно хаос повлиял)
4. Записывает verdict (Pass/Fail)

### Определение временных окон

```
        baselineWindow (30m)          experiment duration
    ◄──────────────────────────►  ◄──────────────────────────►
    │                           │  │                           │
────┼───────────────────────────┼──┼───────────────────────────┼────
    │      BASELINE metrics     │  │     CHAOS metrics         │
    │      (спокойное состояние)│  │     (под нагрузкой)       │
    │                           │  │                           │
    t0                    startedAt                      completedAt
```

- **Baseline window**: `[startedAt - baselineWindow, startedAt]` — задаётся пользователем
- **Chaos window**: `[startedAt, completedAt]` — автоматически из status эксперимента

### CRD: ChaosAnalysis

```yaml
apiVersion: chaos.io/v1
kind: ChaosAnalysis
metadata:
  name: latency-check
spec:
  experimentRef:
    name: pod-killer-test
    namespace: default

  prometheus:
    url: "http://prometheus:9090"
    baselineWindow: 30m

  query: |
    histogram_quantile(0.99,
      rate(http_request_duration_seconds_bucket{service="my-app"}[5m])
    )

  degradationDirection: up     # up = рост значения это плохо (latency, errors)
                               # down = падение значения это плохо (throughput)

  successCriteria:
    maxImpact: 30              # максимальный допустимый impact для Pass

status:
  phase: Completed
  verdict: Pass
  impactScore: 15
  baselineValue: 0.12
  duringValue: 0.30
  degradationPercent: 150.0
  message: "Impact 15/100. p99 latency: 0.12s → 0.30s (+150%), within threshold (max 30)"
```

### Формула скоринга

```
if direction == "up":
    degradation% = max(0, (during - baseline) / baseline × 100)
if direction == "down":
    degradation% = max(0, (baseline - during) / baseline × 100)

impact_score = clamp(0, degradation%, 100)

verdict = impactScore <= maxImpact ? Pass : Fail
```

0 = нет влияния, метрики не изменились.
100 = максимальное влияние, полная деградация.

### Примеры

| Метрика | Baseline | During | Direction | Degradation% | Impact | maxImpact=30 |
|---------|----------|--------|-----------|-------------|--------|-------------|
| p99 latency | 0.10s | 0.10s | up | 0% | 0 | Pass |
| p99 latency | 0.10s | 0.15s | up | 50% | 50 | Fail |
| p99 latency | 0.10s | 0.13s | up | 30% | 30 | Pass |
| throughput | 1000 rps | 800 rps | down | 20% | 20 | Pass |
| throughput | 1000 rps | 200 rps | down | 80% | 80 | Fail |

### Несколько анализов на один эксперимент

```yaml
# latency-check.yaml — direction: up, maxImpact: 30
# error-rate-check.yaml — direction: up, maxImpact: 10
# throughput-check.yaml — direction: down, maxImpact: 20
```

### Reconcile Flow для ChaosAnalysis

```
ChaosAnalysis created
        │
        ▼
  Find referenced ChaosExperiment
        │
        ├── not found or not completed → phase: Pending, requeue
        │
        ▼
  Get startedAt, completedAt from experiment status
        │
        ▼
  Query Prometheus: baseline window
        │
        ▼
  Query Prometheus: chaos window (startedAt → completedAt)
        │
        ▼
  Calculate degradation% and score
        │
        ▼
  verdict = impactScore <= maxImpact ? Pass : Fail
        │
        ▼
  Write to status + export chaos_impact_score metric
```

## 9. Метрики (Prometheus)

### Operator metrics (`/metrics` на operator pod)

| Метрика | Тип | Labels | Описание |
|---------|-----|--------|----------|
| chaos_experiments_total | Counter | phase, scenario | Количество экспериментов |
| chaos_experiment_duration_seconds | Histogram | scenario | Длительность |
| chaos_experiment_info | Gauge | name, namespace, scenario, phase | Текущее состояние |
| chaos_runner_jobs_total | Counter | scenario, status | Созданные runner Jobs |
| chaos_impact_score | Gauge | experiment, namespace, scenario, analysis | Impact score 0-100 |

### Runner metrics (`/metrics` на runner pod, :9090)

| Метрика | Тип | Labels | Описание |
|---------|-----|--------|----------|
| chaos_injection_active | Gauge | experiment_id, scenario | 1 если инъекция активна |
| chaos_injection_total | Counter | scenario, result | Выполненные инъекции |
| chaos_targets_affected | Gauge | experiment_id | Затронутые цели |
| chaos_injection_duration_seconds | Histogram | scenario | Время инъекции |

Runner pods аннотированы `prometheus.io/scrape: "true"` для автоматического scrape.

## 10. Type-Driven Design (Rust)

- Newtype wrappers: `ExperimentDuration(NonZeroU64)`, `ExperimentId(Uuid)`
- Typed errors: `ValidationError`, `RunnerError` — enum-ы, не строки
- Phase и ScenarioType — enum-ы
- Никаких `unwrap()`, `expect()`, `panic!()` в production коде
- Все ошибки через `Result`/`Option` с `?` propagation

## 11. Конфигурация

### Operator

| Переменная | Default | Описание |
|------------|---------|----------|
| RUNNER_IMAGE | chimp-chaos:latest | Docker image для runner Jobs |
| RECONCILE_INTERVAL | 10s | Интервал reconcile |
| RUNNER_METRICS_PORT | 9090 | Порт метрик runner-а |
| PROMETHEUS_URL | http://prometheus:9090 | Prometheus для GraphBuilder |
| GRAPH_LOOKBACK | 10m | Окно для построения observed graph |
| GRAPH_MIN_RPS | 0.05 | Минимальный RPS для валидного edge |

### Runner (передаются оператором через env)

| Переменная | Описание |
|------------|----------|
| EXPERIMENT_ID | UUID эксперимента |
| SCENARIO | PodKiller / CpuStress / NetworkDelay (runner-only сценарии) |
| DURATION | Длительность в секундах |
| PARAMETERS | JSON с параметрами сценария |
| METRICS_PORT | Порт для /metrics (default 9090) |

## 12. Структура проекта

```
src/
├── main.rs                  # CLI: --mode operator | runner
├── operator/
│   ├── mod.rs
│   ├── crd.rs               # ChaosExperiment, ChaosAnalysis CRD types
│   ├── reconciler.rs        # Reconcile logic: create Jobs / Istio policies
│   ├── job_builder.rs       # Construct Job specs (RunnerJobInjector)
│   ├── istio_injector.rs    # Create/delete Istio VirtualService (IstioEdgeInjector)
│   ├── graph_builder.rs     # On-demand observed graph from Prometheus
│   └── types.rs             # Newtype wrappers, typed errors
└── runner/
    ├── mod.rs
    ├── server.rs            # HTTP server: /metrics, /health
    ├── metrics.rs           # Prometheus metrics registry
    └── scenarios/
        ├── mod.rs
        ├── pod_killer.rs    # K8s API pod delete
        ├── cpu_stress.rs    # stress-ng wrapper
        └── network_delay.rs # tc netem wrapper

examples/
├── pod-killer.yaml
├── cpu-stress.yaml
├── network-delay.yaml
├── edge-delay.yaml          # Edge chaos example
├── edge-abort.yaml          # Edge chaos example
└── analysis-latency.yaml    # ChaosAnalysis example

docs/
└── SPEC.md                  # этот файл
```

## 13. RBAC

### Operator ServiceAccount

- ChaosExperiment: get, list, watch, patch (status)
- ChaosAnalysis: get, list, watch, patch (status)
- Jobs: create, get, list, watch, delete
- Pods: get, list (для определения целевых нод)
- VirtualServices (networking.istio.io): create, get, list, watch, delete (для edge chaos)

### Runner ServiceAccount

- Pods: get, list, delete (для PodKiller сценария)
- Минимальные права, только то что нужно для конкретного сценария

