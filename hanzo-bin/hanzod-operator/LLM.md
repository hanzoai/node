# hanzod-operator — AI Assistant Context

The Kubernetes-operator surface of hanzod: watches `services.hanzo.ai/v1`
(`kind: Service`) and reconciles each CR to Deployment + Service + optional
Ingress; leaderless across clusters via the `Coordinator` seam.

The full design — module layout, reconcile model, and the `hanzoai/ha` + Lux ZAP
consensus seam with the roadmap to full leaderless-BFT — lives in the repo root
`LLM.md`, section "hanzod Kubernetes Operator". This file is a pointer to it (one
source of truth, no duplication).
