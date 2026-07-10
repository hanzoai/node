//! The canonical managed-CRD bundle, in one place.
//!
//! Both the `generate-crd-yaml` binary (which renders the checked-in
//! `k8s/crds/all-<group>.yaml` per universe) and the `install` path (which
//! applies the CRDs directly into a cluster) need the exact same set of Kinds
//! at the exact same group. Defining it once here keeps them DRY — there is no
//! second list of Kinds to drift.

use k8s_openapi::apiextensions_apiserver::pkg::apis::apiextensions::v1::CustomResourceDefinition;
use kube::CustomResourceExt;

use crate::api_group::DEFAULT_API_GROUP;
use crate::crd::{
    AgentDeployment, Base, Chain, Datastore, DocDB, Explorer, Function, Gateway, Indexer, Ingress,
    LuxRuntime, ManagedDatabase, Network, NodeFleet, Observability, Queue, Service, Static,
    Validator, DNS, IAM, KMS, KV, LLM, MPC, S3, SPA, SQL,
};

/// Returns every managed CRD in canonical bundle order, with the group already
/// rewritten to `group`. Kept as a function (not inline in a `main`) so both
/// binaries and the unit tests can consume it without spawning a process.
///
/// Order is identical to `bootnode/operator` config/crd/bases and to the
/// checked-in `k8s/crds/all-*.yaml` bundles.
pub fn bundle(group: &str) -> Vec<CustomResourceDefinition> {
    let mut crds = vec![
        Service::crd(),
        Datastore::crd(),
        Gateway::crd(),
        MPC::crd(),
        Network::crd(),
        Ingress::crd(),
        DNS::crd(),
        Base::crd(),
        SQL::crd(),
        KV::crd(),
        DocDB::crd(),
        IAM::crd(),
        KMS::crd(),
        LLM::crd(),
        S3::crd(),
        ManagedDatabase::crd(),
        Chain::crd(),
        Validator::crd(),
        Indexer::crd(),
        Explorer::crd(),
        SPA::crd(),
        Static::crd(),
        Queue::crd(),
        Observability::crd(),
        Function::crd(),
        LuxRuntime::crd(),
        NodeFleet::crd(),
        AgentDeployment::crd(),
    ];

    if group != DEFAULT_API_GROUP {
        for crd in &mut crds {
            rewrite_group(crd, group);
        }
    }
    crds
}

/// Rewrites a single CRD's group from the compile-time default to `group`.
/// Touches `spec.group` and the derived `metadata.name = <plural>.<group>`
/// (the only two places the group string appears in a kube-rs-generated CRD).
pub fn rewrite_group(crd: &mut CustomResourceDefinition, group: &str) {
    let plural = crd.spec.names.plural.clone();
    crd.spec.group = group.to_string();
    crd.metadata.name = Some(format!("{plural}.{group}"));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundle_is_the_canonical_28_kind_set() {
        let crds = bundle(DEFAULT_API_GROUP);
        assert_eq!(crds.len(), 28, "managed Kind count must stay at 28");

        // AgentDeployment (the autonomous-bot Kind) is in the bundle.
        let kinds: Vec<&str> = crds.iter().map(|c| c.spec.names.kind.as_str()).collect();
        assert!(
            kinds.contains(&"AgentDeployment"),
            "AgentDeployment Kind must be present",
        );
        let ad = crds
            .iter()
            .find(|c| c.spec.names.kind == "AgentDeployment")
            .expect("AgentDeployment CRD");
        assert_eq!(ad.spec.names.plural, "agentdeployments");
        assert_eq!(
            ad.metadata.name.as_deref(),
            Some("agentdeployments.hanzo.ai")
        );
        // `bot` shortname — a Bot is what it materializes.
        assert!(ad
            .spec
            .names
            .short_names
            .as_deref()
            .map(|s| s.contains(&"bot".to_string()))
            .unwrap_or(false));

        // Every Kind carries the default group and a well-formed metadata.name.
        for crd in &crds {
            assert_eq!(crd.spec.group, DEFAULT_API_GROUP);
            let plural = &crd.spec.names.plural;
            assert_eq!(
                crd.metadata.name.as_deref(),
                Some(format!("{plural}.{DEFAULT_API_GROUP}").as_str()),
            );
        }
    }

    #[test]
    fn bundle_is_compat_free_and_base_is_bare_named() {
        let crds = bundle(DEFAULT_API_GROUP);
        let kinds: Vec<&str> = crds.iter().map(|c| c.spec.names.kind.as_str()).collect();

        // Bare `Base` — not the old `BaseApp` — at `bases.hanzo.ai`.
        assert!(kinds.contains(&"Base"), "Base Kind must be present");
        assert!(
            !kinds.contains(&"BaseApp"),
            "legacy BaseApp Kind must be gone (renamed to Base)",
        );
        let base = crds
            .iter()
            .find(|c| c.spec.names.kind == "Base")
            .expect("Base CRD");
        assert_eq!(base.spec.names.plural, "bases");
        assert_eq!(base.spec.names.singular.as_deref(), Some("base"));
        assert_eq!(base.metadata.name.as_deref(), Some("bases.hanzo.ai"));

        // NO Hanzo-prefixed compat alias Kinds survive.
        for k in &kinds {
            assert!(
                !k.starts_with("Hanzo"),
                "compat-free: no Hanzo-prefixed Kind allowed, found {k}",
            );
        }
        assert!(!kinds.contains(&"HanzoService"));
        assert!(!kinds.contains(&"HanzoDatastore"));
        assert!(!kinds.contains(&"HanzoDNS"));
    }

    #[test]
    fn luxruntime_is_present_and_lux_network_is_gone() {
        let crds = bundle(DEFAULT_API_GROUP);
        let kinds: Vec<&str> = crds.iter().map(|c| c.spec.names.kind.as_str()).collect();
        assert!(
            kinds.contains(&"LuxRuntime"),
            "LuxRuntime Kind must be present for bootnode parity",
        );
        assert!(
            !kinds.contains(&"LuxNetwork"),
            "legacy LuxNetwork Kind must not be emitted",
        );

        let lrt = crds
            .iter()
            .find(|c| c.spec.names.kind == "LuxRuntime")
            .expect("LuxRuntime CRD");
        assert_eq!(lrt.spec.names.plural, "luxruntimes");
        assert_eq!(lrt.spec.names.singular.as_deref(), Some("luxruntime"));
        assert_eq!(
            lrt.spec.names.short_names.as_deref(),
            Some(["lrt".to_string()].as_slice()),
            "LuxRuntime shortname must match bootnode canonical `lrt`",
        );
        assert_eq!(lrt.metadata.name.as_deref(), Some("luxruntimes.hanzo.ai"));
    }

    #[test]
    fn group_rewrite_touches_spec_group_and_metadata_name() {
        let crds = bundle("lux.cloud");
        for crd in &crds {
            assert_eq!(crd.spec.group, "lux.cloud");
            let plural = &crd.spec.names.plural;
            assert_eq!(
                crd.metadata.name.as_deref(),
                Some(format!("{plural}.lux.cloud").as_str()),
            );
        }
        // Spot-check the renamed Kind survives the rewrite.
        let lrt = crds
            .iter()
            .find(|c| c.spec.names.kind == "LuxRuntime")
            .expect("LuxRuntime CRD");
        assert_eq!(lrt.metadata.name.as_deref(), Some("luxruntimes.lux.cloud"));
    }
}
