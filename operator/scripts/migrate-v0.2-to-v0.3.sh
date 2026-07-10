#!/usr/bin/env bash
# Migrate v0.2.x CRs to v0.3.0 schema.
#
# v0.2.x had legacy compat Kinds (HanzoService/HanzoDatastore/HanzoDNS).
# v0.3.0+ collapses those to the unprefixed canonical Kinds:
#   HanzoService    → Service
#   HanzoDatastore  → Datastore
#   HanzoDNS        → DNS
#
# The BaseApp Kind keeps its canonical name across v0.2 → v0.4 (it matches
# the `bootno.de/v1` canonical `BaseApp`), so no Kind rewrite is needed for
# BaseApp CRs.
#
# This script rewrites every legacy CR in the cluster to its unprefixed
# equivalent. Idempotent. Defaults to dry-run; set DRY_RUN=false to apply.
#
# Run BEFORE deploying ghcr.io/hanzoai/operator:v0.3.0 to cluster.
set -euo pipefail

DRY_RUN="${DRY_RUN:-true}"
CONTEXT="${KUBE_CONTEXT:-do-sfo3-hanzo-k8s}"
NAMESPACE="${NAMESPACE:--A}"  # -A means all namespaces

if [ "$DRY_RUN" = "true" ]; then
  APPLY_FLAGS="--dry-run=client -o yaml"
  echo "DRY RUN — no changes applied. Set DRY_RUN=false to apply."
else
  APPLY_FLAGS=""
fi

migrate_kind() {
  local OLD="$1"
  local NEW="$2"
  echo "=== ${OLD} → ${NEW} ==="

  if ! kubectl --context="${CONTEXT}" get "${OLD}.hanzo.ai" ${NAMESPACE} >/dev/null 2>&1; then
    echo "  no ${OLD} CRs found, skipping"
    return
  fi

  kubectl --context="${CONTEXT}" get "${OLD}.hanzo.ai" ${NAMESPACE} -o json \
    | jq --arg new "${NEW}" '
        .items[]
        | .kind = $new
        | .apiVersion = "hanzo.ai/v1"
        | del(.status, .metadata.resourceVersion, .metadata.uid,
              .metadata.generation, .metadata.creationTimestamp,
              .metadata.managedFields, .metadata.ownerReferences)
      ' \
    | kubectl --context="${CONTEXT}" apply ${APPLY_FLAGS} -f -
}

migrate_kind HanzoService    Service
migrate_kind HanzoDatastore  Datastore
migrate_kind HanzoDNS        DNS

if [ "$DRY_RUN" = "false" ]; then
  echo ""
  echo "=== Migration complete. Delete legacy CRs once v0.3.0 is verified live: ==="
  echo "  kubectl --context=${CONTEXT} delete hanzoservices.hanzo.ai -A"
  echo "  kubectl --context=${CONTEXT} delete hanzodatastores.hanzo.ai -A"
  echo "  kubectl --context=${CONTEXT} delete hanzodns.hanzo.ai -A"
fi
