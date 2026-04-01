# Chimp Chaos Dashboard

## Обзор

Веб-приложение для визуализации сервис-графа и управления chaos экспериментами. Работает в браузере. Состоит из двух частей:

- **Backend** — `chimp-chaos --mode dashboard` — actix-web API сервер, проксирует K8s API и Prometheus, раздаёт WASM фронтенд
- **Frontend** — egui WASM приложение в `dashboard/` workspace member, компилируется с `trunk`

## Запуск

```bash
# 1. Собрать WASM фронтенд
cd dashboard && trunk build --release && cd ..

# 2. Port-forward к Prometheus (если нет прямого доступа)
kubectl port-forward -n monitoring svc/kube-prometheus-stack-prometheus 9090:9090 &

# 3. Запустить backend (раздаёт API + WASM)
PROMETHEUS_URL=http://localhost:9090 cargo run --bin chimp-chaos -- --mode dashboard

# 4. Открыть в браузере
open http://localhost:8080
```

## Архитектура

```
┌─────────────┐      ┌──────────────────────────┐      ┌─────────────┐
│   Browser   │─────▶│  --mode dashboard        │─────▶│ Prometheus  │
│  (egui WASM)│      │  (actix-web :8080)       │      └─────────────┘
│             │◀─────│                           │
│  egui_graphs│      │  GET  /api/graph          │─────▶┌─────────────┐
│  widget     │      │  GET  /api/experiments    │      │  K8s API    │
│             │      │  POST /api/experiments    │      │ (kubeconfig)│
│             │      │  GET  /api/analyses       │◀─────└─────────────┘
│             │      │  POST /api/analyses       │
│             │      │  GET  / (serves WASM app) │
└─────────────┘      └──────────────────────────┘
```

Dashboard НЕ общается с оператором напрямую. Он создаёт CRD ресурсы через backend API, а оператор их reconcile-ит.

## Backend API (`--mode dashboard`)

### Endpoints

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/` | Статические файлы WASM app (из `dashboard/dist/`) |
| `GET` | `/api/graph?namespace=demo` | Граф сервисов из Prometheus `istio_requests_total` |
| `GET` | `/api/experiments?namespace=default` | Список ChaosExperiment |
| `POST` | `/api/experiments` | Создать ChaosExperiment |
| `GET` | `/api/analyses?namespace=default` | Список ChaosAnalysis |
| `POST` | `/api/analyses` | Создать ChaosAnalysis |

### GET /api/graph

Запрашивает Prometheus, возвращает JSON:

```json
{
  "nodes": [
    {"id": "frontend", "namespace": "demo"},
    {"id": "cartservice", "namespace": "demo"}
  ],
  "edges": [
    {
      "source": "frontend",
      "destination": "cartservice",
      "namespace": "demo",
      "rps": 2.5
    }
  ]
}
```

### POST /api/experiments

Body:

```json
{
  "name": "edge-delay-test",
  "namespace": "default",
  "scenario": "EdgeDelay",
  "duration": 120,
  "targetNamespace": "demo",
  "target": {
    "edge": {
      "sourceService": "frontend",
      "destinationService": "cartservice"
    }
  },
  "parameters": {"latencyMs": 500}
}
```

## Frontend (egui WASM)

### Структура

```
dashboard/
├── Cargo.toml
├── index.html          # HTML shell для trunk
└── src/
    ├── main.rs         # eframe WASM entry point
    ├── app.rs          # App struct, UI layout, update loop
    ├── api.rs          # HTTP клиент (ehttp для WASM)
    └── graph.rs        # Построение petgraph из API данных
```

### Зависимости

| Crate | Version | Purpose |
|-------|---------|---------|
| `eframe` | 0.33 | egui framework (WASM target) |
| `egui_graphs` | 0.29 | Interactive graph widget |
| `petgraph` | 0.7 | Graph data structure |
| `ehttp` | 0.5 | HTTP client (works in WASM) |
| `serde` + `serde_json` | 1 | JSON parsing |

### Функциональность

#### 1. Service Graph (центральная область)

- Граф строится из `/api/graph` данных
- Ноды = workloads (source_workload, destination_workload)
- Рёбра = трафик между workloads (RPS на label)
- Force-directed layout через egui_graphs
- Drag, zoom, pan из коробки
- Автообновление по таймеру (каждые 30с)
- Кнопка Refresh для ручного обновления

#### 2. Клик по ребру → Edge Chaos

- source/destination workload, namespace, текущий RPS
- Выбор: EdgeDelay / EdgeAbort
- Параметры: latencyMs / abortPercent, abortHttpStatus
- Duration (30-600s)
- Кнопка "Run Experiment" → POST /api/experiments

#### 3. Клик по ноде → Pod/Node Chaos

- workload name, namespace
- Выбор: PodKiller / CpuStress / NetworkDelay
- Параметры в зависимости от сценария
- Кнопка "Run Experiment"

#### 4. Боковая панель — Experiments & Analysis

- Список ChaosExperiment с фазами (цветовая индикация)
- Список ChaosAnalysis с verdict/impact
- Кнопка "Create Analysis" для завершённых экспериментов

### UI Layout

```
┌──────────────────────────────────────────────────────────┐
│  [Refresh] [Namespace: ▼demo]      Chimp Chaos Dashboard │
├──────────────────────────────────┬────────────────────────┤
│                                  │  Experiments           │
│                                  │  ┌──────────────────┐  │
│        Service Graph             │  │ pod-killer-1  ● S │  │
│     (egui_graphs widget)         │  │ edge-delay-2  ● R │  │
│                                  │  │ cpu-stress-3  ● F │  │
│  [frontend] ──→ [cartservice]    │  └──────────────────┘  │
│       │                          │                        │
│       ▼                          │  Analysis Results      │
│  [productcatalog]                │  ┌──────────────────┐  │
│                                  │  │ impact: 15  Pass │  │
│  клик по ребру → панель снизу    │  │ impact: 80  Fail │  │
│                                  │  └──────────────────┘  │
├──────────────────────────────────┴────────────────────────┤
│  Edge: frontend → cartservice (2.5 rps)                   │
│  Scenario: [EdgeDelay ▼]  Latency: [===200ms===]          │
│  Duration: [===60s===]                [Run Experiment]     │
└──────────────────────────────────────────────────────────┘
```

## Build

### Frontend (WASM)

```bash
# Установить trunk (один раз)
cargo install trunk

# Собрать
cd dashboard
trunk build --release
# Output: dashboard/dist/
```

### Backend

```bash
cargo build --bin chimp-chaos --release
```

### Docker

Dockerfile собирает оба:
1. Компилирует WASM фронтенд с trunk
2. Компилирует бинарник
3. Копирует dist/ в образ
