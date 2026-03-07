#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT_DIR="$(dirname "$SCRIPT_DIR")"
AWS_PROFILE="kirill"
AWS_REGION="eu-central-1"

# Get ECR URL from terraform output
ECR_URI=$(cd "$ROOT_DIR/infra" && terraform output -raw ecr_repository_url)
IMAGE_TAG="${IMAGE_TAG:-latest}"
FULL_IMAGE="${ECR_URI}:${IMAGE_TAG}"

echo "=== 1. ECR login ==="
aws ecr get-login-password --region "$AWS_REGION" --profile "$AWS_PROFILE" | \
    docker login --username AWS --password-stdin "$ECR_URI"

echo ""
echo "=== 2. Build Docker image ==="
docker build --platform linux/amd64 -t "$FULL_IMAGE" "$ROOT_DIR"

echo ""
echo "=== 3. Push to ECR ==="
docker push "$FULL_IMAGE"

echo ""
echo "=== 4. Apply CRDs ==="
kubectl apply -f "$SCRIPT_DIR/crds.yaml"

echo ""
echo "=== 5. Apply RBAC ==="
kubectl apply -f "$SCRIPT_DIR/rbac.yaml"

echo ""
echo "=== 6. Deploy operator ==="
sed "s|image: chimp-chaos:latest|image: ${FULL_IMAGE}|g" "$SCRIPT_DIR/operator.yaml" | \
    sed "s|value: \"chimp-chaos:latest\"|value: \"${FULL_IMAGE}\"|g" | \
    kubectl apply -f -

echo ""
echo "=== 7. Waiting for rollout ==="
kubectl rollout status deployment/chimp-chaos-operator --timeout=120s

echo ""
echo "=== Done! ==="
echo "  kubectl logs -f deployment/chimp-chaos-operator"
echo "  kubectl apply -f examples/pod-killer.yaml"
echo "  kubectl get ce -w"
