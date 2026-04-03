#!/usr/bin/env bash
set -euo pipefail

FIXTURE_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/fixtures" && pwd)"
OPERATOR_PID=""
PF_PID=""

cleanup() {
  echo "==> Cleanup..."
  kubectl delete -f "$FIXTURE_DIR/" --ignore-not-found 2>/dev/null || true
  [ -n "$OPERATOR_PID" ] && kill "$OPERATOR_PID" 2>/dev/null || true
  [ -n "$PF_PID" ] && kill "$PF_PID" 2>/dev/null || true
}
trap cleanup EXIT

# --- Port-forward Prometheus ---
echo "==> Port-forwarding Prometheus..."
kubectl port-forward svc/prometheus-kube-prometheus-stack-prometheus -n monitoring 9090:9090 &
PF_PID=$!
sleep 2

# --- Start operator ---
echo "==> Starting operator..."
RUST_LOG=info \
PROMETHEUS_URL=http://localhost:9090 \
RUNNER_IMAGE="${RUNNER_IMAGE:-kirill02102/ch-chaos:latest}" \
cargo run -- --mode operator &
OPERATOR_PID=$!
sleep 5

# --- Apply experiment ---
echo "==> Applying pod-killer experiment..."
kubectl apply -f "$FIXTURE_DIR/pod-killer-cartservice.yaml"

# --- Wait for experiment ---
echo "==> Waiting for experiment to complete (timeout 120s)..."
for i in $(seq 1 24); do
  PHASE=$(kubectl get ce e2e-pod-killer -n online-boutique -o jsonpath='{.status.phase}' 2>/dev/null || echo "Pending")
  echo "  phase: $PHASE"
  if [ "$PHASE" = "Succeeded" ] || [ "$PHASE" = "Failed" ]; then
    break
  fi
  sleep 5
done

# --- Show experiment result ---
echo ""
echo "==> Experiment result:"
kubectl get ce e2e-pod-killer -n online-boutique -o yaml | grep -A 20 "^status:"

# --- Apply analysis ---
echo ""
echo "==> Applying analysis..."
kubectl apply -f "$FIXTURE_DIR/analysis-cartservice.yaml"

echo "==> Waiting for analysis to complete (timeout 60s)..."
for i in $(seq 1 12); do
  PHASE=$(kubectl get ca e2e-analysis -n online-boutique -o jsonpath='{.status.phase}' 2>/dev/null || echo "Pending")
  echo "  phase: $PHASE"
  if [ "$PHASE" = "Completed" ] || [ "$PHASE" = "Failed" ]; then
    break
  fi
  sleep 5
done

# --- Show analysis result ---
echo ""
echo "==> Analysis result:"
kubectl get ca e2e-analysis -n online-boutique -o yaml | grep -A 20 "^status:"

echo ""
echo "==> Done."
