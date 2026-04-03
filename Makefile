IMAGE := kirill02102/ch-chaos
TAG   ?= dev

.PHONY: test lint fmt check run run-port-forward docker-build docker-push crds e2e

# --- Dev cycle ---
test:
	cargo test

lint:
	cargo clippy -- -D warnings

fmt:
	cargo fmt --all

check: fmt lint test

# --- Local operator ---
run:
	RUST_LOG=debug \
	PROMETHEUS_URL=http://localhost:9090 \
	RUNNER_IMAGE=$(IMAGE):$(TAG) \
	cargo run --bin chimp-chaos -- --mode operator

run-port-forward:
	kubectl port-forward svc/prometheus-kube-prometheus-stack-prometheus -n monitoring 9090:9090

# --- Docker ---
docker-build:
	docker build -t $(IMAGE):$(TAG) .

docker-push: docker-build
	docker push $(IMAGE):$(TAG)

# --- E2E ---
e2e:
	RUNNER_IMAGE=$(IMAGE):$(TAG) ./tests/e2e/run.sh

# --- CRDs ---
crds:
	cargo run --bin crd_gen > deploy/crds.yaml
