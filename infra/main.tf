terraform {
  required_version = ">= 1.5.0"

  required_providers {
    aws = {
      source  = "hashicorp/aws"
      version = ">= 5.0"
    }
    kubernetes = {
      source  = "hashicorp/kubernetes"
      version = ">= 2.29"
    }
    helm = {
      source  = "hashicorp/helm"
      version = ">= 2.13"
    }
    random = {
      source  = "hashicorp/random"
      version = ">= 3.6"
    }
    github = {
      source  = "integrations/github"
      version = "~> 6.0"
    }
  }
}

provider "aws" {
  region  = "eu-central-1"
  profile = "kirill"
}

provider "github" {
  owner = "kirkylesich"
}

data "aws_availability_zones" "this" {
  state = "available"
}

locals {
  name            = "eks-main"
  azs             = slice(data.aws_availability_zones.this.names, 0, 2)
  vpc_cidr        = "10.0.0.0/16"
  public_subnets  = ["10.0.0.0/20", "10.0.16.0/20"]
  private_subnets = ["10.0.32.0/19", "10.0.64.0/19"]
}

# ── VPC ──

module "vpc" {
  source  = "terraform-aws-modules/vpc/aws"
  version = "~> 5.0"

  name = local.name
  cidr = local.vpc_cidr

  azs             = local.azs
  public_subnets  = local.public_subnets
  private_subnets = local.private_subnets

  enable_nat_gateway   = true
  single_nat_gateway   = true
  enable_dns_support   = true
  enable_dns_hostnames = true

  public_subnet_tags = {
    "kubernetes.io/role/elb" = "1"
  }

  private_subnet_tags = {
    "kubernetes.io/role/internal-elb" = "1"
  }

  tags = {
    "kubernetes.io/cluster/${local.name}" = "shared"
  }
}

# ── EKS ──

module "eks" {
  source  = "terraform-aws-modules/eks/aws"
  version = "~> 20.0"

  cluster_name    = local.name
  cluster_version = "1.33"
  vpc_id          = module.vpc.vpc_id
  subnet_ids      = module.vpc.private_subnets

  cluster_endpoint_public_access           = true
  enable_irsa                              = true
  enable_cluster_creator_admin_permissions = true

  cluster_addons = {
    coredns = {
      most_recent = true
    }
    kube-proxy = {
      most_recent = true
    }
    vpc-cni = {
      most_recent = true
    }
  }

  eks_managed_node_groups = {
    ng = {
      name           = "ng-1"
      instance_types = ["t3.large"]
      desired_size   = 3
      min_size       = 3
      max_size       = 3
      capacity_type  = "ON_DEMAND"
      subnet_ids     = module.vpc.private_subnets
      ami_type       = "AL2023_x86_64_STANDARD"
    }
  }
}

# ── Security group: Istio webhook ──

resource "aws_security_group_rule" "eks_cp_to_nodes_istio_webhook" {
  description = "Allow EKS control plane to access Istio sidecar injector (port 15017) on nodes"

  type                     = "ingress"
  from_port                = 15017
  to_port                  = 15017
  protocol                 = "tcp"

  security_group_id        = module.eks.node_security_group_id
  source_security_group_id = module.eks.cluster_security_group_id
}

# ── EBS CSI driver ──

module "ebs_csi_irsa" {
  source  = "terraform-aws-modules/iam/aws//modules/iam-role-for-service-accounts-eks"
  version = "~> 5.0"

  role_name             = "${local.name}-ebs-csi-irsa"
  attach_ebs_csi_policy = true
  oidc_providers = {
    eks = {
      provider_arn = module.eks.oidc_provider_arn
      namespace_service_accounts = [
        "kube-system:ebs-csi-controller-sa"
      ]
    }
  }
}

resource "aws_eks_addon" "ebs_csi" {
  cluster_name                = module.eks.cluster_name
  addon_name                  = "aws-ebs-csi-driver"
  service_account_role_arn    = module.ebs_csi_irsa.iam_role_arn
  resolve_conflicts_on_create = "OVERWRITE"
  resolve_conflicts_on_update = "OVERWRITE"
  depends_on                  = [module.eks, module.ebs_csi_irsa]
}

# ── Providers for k8s/helm ──

data "aws_eks_cluster" "this" {
  name       = module.eks.cluster_name
  depends_on = [module.eks]
}

data "aws_eks_cluster_auth" "this" {
  name       = module.eks.cluster_name
  depends_on = [module.eks]
}

provider "kubernetes" {
  host                   = data.aws_eks_cluster.this.endpoint
  cluster_ca_certificate = base64decode(data.aws_eks_cluster.this.certificate_authority[0].data)
  token                  = data.aws_eks_cluster_auth.this.token
}

provider "helm" {
  kubernetes = {
    host                   = data.aws_eks_cluster.this.endpoint
    cluster_ca_certificate = base64decode(data.aws_eks_cluster.this.certificate_authority[0].data)
    token                  = data.aws_eks_cluster_auth.this.token
  }
}

# ── Metrics Server ──

resource "helm_release" "metrics_server" {
  name             = "metrics-server"
  repository       = "https://kubernetes-sigs.github.io/metrics-server/"
  chart            = "metrics-server"
  namespace        = "kube-system"
  create_namespace = false

  values = [
    <<-YAML
    args:
      - --kubelet-insecure-tls
      - --kubelet-preferred-address-types=InternalIP,Hostname,ExternalIP
    YAML
  ]

  depends_on = [module.eks]
}

# ── Istio ──

resource "kubernetes_namespace_v1" "istio_system" {
  metadata {
    name = "istio-system"
  }
  depends_on = [module.eks]
}

resource "helm_release" "istio_base" {
  name             = "istio-base"
  repository       = "https://istio-release.storage.googleapis.com/charts"
  chart            = "base"
  version          = "1.29.0"
  namespace        = "istio-system"
  create_namespace = false

  depends_on = [kubernetes_namespace_v1.istio_system]
}

resource "helm_release" "istiod" {
  name             = "istiod"
  repository       = "https://istio-release.storage.googleapis.com/charts"
  chart            = "istiod"
  version          = "1.29.0"
  namespace        = "istio-system"
  create_namespace = false

  values = [
    <<-YAML
    meshConfig:
      enablePrometheusMerge: true
      defaultConfig:
        holdApplicationUntilProxyStarts: true
        proxyMetadata:
          ISTIO_META_DNS_CAPTURE: "true"
          ISTIO_META_DNS_AUTO_ALLOCATE: "true"
    pilot:
      resources:
        requests:
          cpu: 100m
          memory: 128Mi
    YAML
  ]

  depends_on = [helm_release.istio_base]
}

# ── Prometheus (kube-prometheus-stack) ──

resource "kubernetes_namespace_v1" "monitoring" {
  metadata {
    name = "monitoring"
  }
  depends_on = [module.eks]
}

resource "helm_release" "kube_prometheus_stack" {
  name             = "kube-prometheus-stack"
  repository       = "https://prometheus-community.github.io/helm-charts"
  chart            = "kube-prometheus-stack"
  namespace        = "monitoring"
  create_namespace = false

  values = [
    <<-YAML
    grafana:
      adminPassword: "admin"

    prometheus:
      prometheusSpec:
        podMonitorSelectorNilUsesHelmValues: false
        serviceMonitorSelectorNilUsesHelmValues: false

    # Istio control plane metrics (istiod, port 15014)
    additionalServiceMonitors:
      - name: istiod
        namespaceSelector:
          matchNames:
            - istio-system
        selector:
          matchLabels:
            app: istiod
        endpoints:
          - port: http-monitoring
            path: /metrics
            interval: 15s
            scrapeTimeout: 10s

    # Istio sidecar proxy metrics (Envoy, port 15020)
    # Selects all pods with Istio sidecar injection via tlsMode label
    additionalPodMonitors:
      - name: istio-proxies
        namespaceSelector:
          any: true
        selector:
          matchExpressions:
            - key: security.istio.io/tlsMode
              operator: Exists
        podMetricsEndpoints:
          - path: /stats/prometheus
            port: http-envoy-prom
            interval: 15s
            scrapeTimeout: 10s
            relabelings:
              - sourceLabels: [__meta_kubernetes_pod_name]
                targetLabel: pod
              - sourceLabels: [__meta_kubernetes_namespace]
                targetLabel: namespace
    YAML
  ]

  depends_on = [kubernetes_namespace_v1.monitoring, helm_release.istiod]
}

# ── Online Boutique (test workloads with Istio sidecars) ──

resource "kubernetes_namespace_v1" "demo" {
  metadata {
    name = "demo"
    labels = {
      "istio-injection" = "enabled"
    }
  }
  depends_on = [helm_release.istiod]
}

resource "null_resource" "online_boutique" {
  triggers = {
    namespace = kubernetes_namespace_v1.demo.metadata[0].name
  }

  provisioner "local-exec" {
    command = <<-EOT
      aws eks update-kubeconfig --name ${module.eks.cluster_name} --region eu-central-1 --profile kirill --kubeconfig /tmp/chimp-chaos-kubeconfig
      KUBECONFIG=/tmp/chimp-chaos-kubeconfig kubectl apply -n demo -f https://raw.githubusercontent.com/GoogleCloudPlatform/microservices-demo/main/release/kubernetes-manifests.yaml
      KUBECONFIG=/tmp/chimp-chaos-kubeconfig kubectl rollout status deployment/frontend -n demo --timeout=300s
    EOT
  }

  provisioner "local-exec" {
    when    = destroy
    command = <<-EOT
      KUBECONFIG=/tmp/chimp-chaos-kubeconfig kubectl delete -n demo -f https://raw.githubusercontent.com/GoogleCloudPlatform/microservices-demo/main/release/kubernetes-manifests.yaml --ignore-not-found
    EOT
  }

  depends_on = [kubernetes_namespace_v1.demo, helm_release.istiod]
}

# ── ECR repository for chimp-chaos ──

resource "aws_ecr_repository" "chimp_chaos" {
  name                 = "chimp-chaos"
  image_tag_mutability = "MUTABLE"
  force_delete         = true

  image_scanning_configuration {
    scan_on_push = false
  }
}

# ── GitHub Actions OIDC → ECR ──

data "aws_caller_identity" "this" {}

resource "aws_iam_openid_connect_provider" "github" {
  url             = "https://token.actions.githubusercontent.com"
  client_id_list  = ["sts.amazonaws.com"]
  thumbprint_list = ["ffffffffffffffffffffffffffffffffffffffff"]
}

resource "aws_iam_role" "github_actions_ecr" {
  name = "github-actions-ecr"

  assume_role_policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Effect = "Allow"
      Principal = {
        Federated = aws_iam_openid_connect_provider.github.arn
      }
      Action = "sts:AssumeRoleWithWebIdentity"
      Condition = {
        StringEquals = {
          "token.actions.githubusercontent.com:aud" = "sts.amazonaws.com"
        }
        StringLike = {
          "token.actions.githubusercontent.com:sub" = "repo:kirkylesich/ch-chaos:*"
        }
      }
    }]
  })
}

resource "aws_iam_role_policy" "github_actions_ecr" {
  name = "ecr-push"
  role = aws_iam_role.github_actions_ecr.id

  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      {
        Effect   = "Allow"
        Action   = "ecr:GetAuthorizationToken"
        Resource = "*"
      },
      {
        Effect = "Allow"
        Action = [
          "ecr:BatchCheckLayerAvailability",
          "ecr:GetDownloadUrlForLayer",
          "ecr:BatchGetImage",
          "ecr:PutImage",
          "ecr:InitiateLayerUpload",
          "ecr:UploadLayerPart",
          "ecr:CompleteLayerUpload",
        ]
        Resource = aws_ecr_repository.chimp_chaos.arn
      },
    ]
  })
}

# ── GitHub Actions Variables (auto-sync) ──

resource "github_actions_variable" "aws_role_arn" {
  repository    = "ch-chaos"
  variable_name = "AWS_ROLE_ARN"
  value         = aws_iam_role.github_actions_ecr.arn
}

resource "github_actions_variable" "aws_region" {
  repository    = "ch-chaos"
  variable_name = "AWS_REGION"
  value         = "eu-central-1"
}

# ── Outputs ──

output "cluster_name" {
  value = module.eks.cluster_name
}

output "cluster_endpoint" {
  value = module.eks.cluster_endpoint
}

output "ecr_repository_url" {
  value = aws_ecr_repository.chimp_chaos.repository_url
}

output "prometheus_internal_url" {
  value = "http://kube-prometheus-stack-prometheus.monitoring.svc.cluster.local:9090"
}

output "kubeconfig_command" {
  value = "aws eks update-kubeconfig --name ${module.eks.cluster_name} --region eu-central-1 --profile kirill"
}
