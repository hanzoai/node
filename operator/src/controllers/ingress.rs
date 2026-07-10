//! Ingress reconciler — multi-domain routing with cert-manager TLS.

use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use k8s_openapi::api::networking::v1::{
    HTTPIngressPath, HTTPIngressRuleValue, Ingress, IngressBackend, IngressRule,
    IngressServiceBackend, IngressSpec as K8sIngressSpec, IngressTLS, ServiceBackendPort,
};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::{ObjectMeta, OwnerReference};
use kube::api::Api;
use kube::runtime::controller::{Action, Controller};
use kube::runtime::watcher::Config;
use kube::{Client, ResourceExt};
use std::collections::BTreeMap;
use tracing::{error, info, warn};

use crate::apply;
use crate::core::{OperatorError, Result};
use crate::crd::{DomainConfig, Ingress as IngressCR, IngressKindSpec};

use super::owner_ref_for;

#[derive(Clone)]
pub struct Ctx {
    pub client: Client,
    pub api_group: String,
}

pub async fn reconcile(cr: Arc<IngressCR>, ctx: Arc<Ctx>) -> Result<Action> {
    let name = cr.name_any();
    let namespace = cr
        .namespace()
        .ok_or_else(|| OperatorError::Config("Ingress has no namespace".into()))?;
    let api_version = format!("{}/v1", ctx.api_group);
    let owner = owner_ref_for(cr.as_ref(), &api_version, "Ingress");
    reconcile_inner(&ctx.client, &name, &namespace, &cr.spec, owner).await?;
    Ok(Action::requeue(Duration::from_secs(60)))
}

async fn reconcile_inner(
    client: &Client,
    name: &str,
    namespace: &str,
    spec: &IngressKindSpec,
    owner: OwnerReference,
) -> Result<()> {
    let class = if spec.ingress_class_name.is_empty() {
        "ingress"
    } else {
        &spec.ingress_class_name
    };
    let issuer = if spec.cluster_issuer.is_empty() {
        "letsencrypt-prod"
    } else {
        &spec.cluster_issuer
    };

    let ings: Api<Ingress> = Api::namespaced(client.clone(), namespace);
    for (idx, domain) in spec.domains.iter().enumerate() {
        let ing = build_domain_ingress(
            name,
            namespace,
            idx,
            domain,
            class,
            issuer,
            spec.annotations.as_ref(),
            spec.labels.as_ref(),
            &owner,
        );
        apply::apply(&ings, &ing).await?;
    }

    info!(
        name,
        namespace,
        domains = spec.domains.len(),
        "Ingress reconciled"
    );
    Ok(())
}

/// Materialize one k8s `Ingress` from a single `DomainConfig`. Pure (no cluster
/// I/O) so it is unit-tested without a kube client — the host, TLS host, issuer
/// annotation, and per-route backends are all asserted here rather than in prod.
///
/// A wildcard `domain` (`*.hanzo.app`, the published-sites edge) is a first-class
/// case: it becomes the k8s Ingress host verbatim (k8s wildcard hosts match
/// exactly one label — `foo.hanzo.app`, never the apex or `a.b.hanzo.app`) and
/// the TLS entry carries the same wildcard so cert-manager issues a `*.hanzo.app`
/// cert via the DNS-01 issuer. No reserved-host exclusion lives here: the backend
/// (cloud `clients/sites`) is the authoritative reserved-label guard.
#[allow(clippy::too_many_arguments)]
fn build_domain_ingress(
    parent_name: &str,
    namespace: &str,
    idx: usize,
    domain: &DomainConfig,
    class: &str,
    issuer: &str,
    extra_annotations: Option<&BTreeMap<String, String>>,
    extra_labels: Option<&BTreeMap<String, String>>,
    owner: &OwnerReference,
) -> Ingress {
    let ing_name = format!("{}-{}-{}", parent_name, idx, sanitize_label(&domain.domain));

    let mut annotations: BTreeMap<String, String> = BTreeMap::new();
    annotations.insert("kubernetes.io/ingress.class".to_string(), class.to_string());
    if domain.tls {
        annotations.insert(
            "cert-manager.io/cluster-issuer".to_string(),
            issuer.to_string(),
        );
    }
    if let Some(extra) = extra_annotations {
        for (k, v) in extra {
            annotations.insert(k.clone(), v.clone());
        }
    }

    let path_type = "Prefix".to_string();
    let paths: Vec<HTTPIngressPath> = domain
        .routes
        .iter()
        .map(|r| HTTPIngressPath {
            path: Some(r.path.clone()),
            path_type: if r.path_type.is_empty() {
                path_type.clone()
            } else {
                r.path_type.clone()
            },
            backend: IngressBackend {
                service: Some(IngressServiceBackend {
                    name: r.service_name.clone(),
                    port: Some(ServiceBackendPort {
                        number: Some(r.service_port),
                        ..Default::default()
                    }),
                }),
                ..Default::default()
            },
        })
        .collect();

    let mut labels: BTreeMap<String, String> = BTreeMap::new();
    labels.insert(
        "app.kubernetes.io/managed-by".to_string(),
        "hanzo-operator".to_string(),
    );
    if let Some(extra) = extra_labels {
        for (k, v) in extra {
            labels.insert(k.clone(), v.clone());
        }
    }

    let tls = if domain.tls {
        Some(vec![IngressTLS {
            hosts: Some(vec![domain.domain.clone()]),
            secret_name: Some(format!("{}-tls", ing_name)),
        }])
    } else {
        None
    };

    Ingress {
        metadata: ObjectMeta {
            name: Some(ing_name),
            namespace: Some(namespace.to_string()),
            labels: Some(labels),
            annotations: Some(annotations),
            owner_references: Some(vec![owner.clone()]),
            ..Default::default()
        },
        spec: Some(K8sIngressSpec {
            rules: Some(vec![IngressRule {
                host: Some(domain.domain.clone()),
                http: Some(HTTPIngressRuleValue { paths }),
            }]),
            tls,
            ..Default::default()
        }),
        ..Default::default()
    }
}

/// k8s object-name-safe rendering of a domain. A wildcard `*` becomes the literal
/// `wildcard` (so `*.hanzo.app` → `wildcard-hanzo-app`, a valid RFC-1123 name)
/// instead of a leading-dash fragment; every other non-alphanumeric byte maps to
/// `-`. Non-wildcard domains are unaffected, so existing Ingress child names never
/// churn.
fn sanitize_label(s: &str) -> String {
    s.replace('*', "wildcard")
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect()
}

pub fn on_error(_obj: Arc<IngressCR>, err: &OperatorError, _ctx: Arc<Ctx>) -> Action {
    error!(error = %err, "Ingress reconcile failed");
    Action::requeue(Duration::from_secs(30))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crd::IngressRoute;

    fn owner() -> OwnerReference {
        OwnerReference {
            api_version: "hanzo.ai/v1".to_string(),
            kind: "Ingress".to_string(),
            name: "hanzo-app-sites".to_string(),
            uid: "test-uid".to_string(),
            ..Default::default()
        }
    }

    fn cloud_route() -> IngressRoute {
        IngressRoute {
            path: "/".to_string(),
            path_type: "Prefix".to_string(),
            service_name: "cloud".to_string(),
            service_port: 8000,
        }
    }

    // The published-sites edge (`*.hanzo.app` → cloud:8000) is the exact CR the
    // universe declares (infra/k8s/operator/crs/hanzo-app-sites.yaml) after the
    // hand-edited routes.yaml router was reverted. These lock that the operator
    // materializes it natively.

    #[test]
    fn wildcard_domain_becomes_the_ingress_host_verbatim() {
        let domain = DomainConfig {
            domain: "*.hanzo.app".to_string(),
            routes: vec![cloud_route()],
            tls: true,
        };
        let ing = build_domain_ingress(
            "hanzo-app-sites",
            "hanzo",
            0,
            &domain,
            "ingress",
            "letsencrypt-prod",
            None,
            None,
            &owner(),
        );
        let rules = ing.spec.unwrap().rules.expect("rules");
        assert_eq!(
            rules[0].host.as_deref(),
            Some("*.hanzo.app"),
            "the wildcard host must be carried verbatim so Traefik matches every single-label subdomain"
        );
    }

    #[test]
    fn wildcard_domain_emits_wildcard_tls_and_issuer() {
        let domain = DomainConfig {
            domain: "*.hanzo.app".to_string(),
            routes: vec![cloud_route()],
            tls: true,
        };
        let ing = build_domain_ingress(
            "hanzo-app-sites",
            "hanzo",
            0,
            &domain,
            "ingress",
            "letsencrypt-prod",
            None,
            None,
            &owner(),
        );
        // cert-manager issuer annotation present (DNS-01 issuer mints the *.hanzo.app cert).
        let ann = ing.metadata.annotations.as_ref().expect("annotations");
        assert_eq!(
            ann.get("cert-manager.io/cluster-issuer")
                .map(String::as_str),
            Some("letsencrypt-prod")
        );
        assert_eq!(
            ann.get("kubernetes.io/ingress.class").map(String::as_str),
            Some("ingress")
        );
        // TLS entry carries the wildcard host so the issued cert is *.hanzo.app.
        let tls = ing.spec.unwrap().tls.expect("tls");
        assert_eq!(
            tls[0].hosts.as_ref().unwrap(),
            &vec!["*.hanzo.app".to_string()]
        );
        // Object-name-safe secret name (no leading-dash fragment from the `*`).
        assert_eq!(
            tls[0].secret_name.as_deref(),
            Some("hanzo-app-sites-0-wildcard-hanzo-app-tls")
        );
    }

    #[test]
    fn wildcard_domain_routes_to_the_declared_backend() {
        let domain = DomainConfig {
            domain: "*.hanzo.app".to_string(),
            routes: vec![cloud_route()],
            tls: true,
        };
        let ing = build_domain_ingress(
            "hanzo-app-sites",
            "hanzo",
            0,
            &domain,
            "ingress",
            "letsencrypt-prod",
            None,
            None,
            &owner(),
        );
        let rules = ing.spec.unwrap().rules.unwrap();
        let path = &rules[0].http.as_ref().unwrap().paths[0];
        assert_eq!(path.path.as_deref(), Some("/"));
        assert_eq!(path.path_type, "Prefix");
        let be = path.backend.service.as_ref().unwrap();
        assert_eq!(be.name, "cloud");
        assert_eq!(be.port.as_ref().unwrap().number, Some(8000));
        // Owner ref set so the child Ingress is GC'd with the CR.
        let refs = ing.metadata.owner_references.expect("owner refs");
        assert_eq!(refs[0].kind, "Ingress");
    }

    #[test]
    fn wildcard_child_name_is_object_safe() {
        // The `*` must not produce a leading-dash name fragment (invalid k8s name).
        assert_eq!(sanitize_label("*.hanzo.app"), "wildcard-hanzo-app");
    }

    #[test]
    fn plain_domain_name_is_unchanged() {
        // Non-wildcard domains must sanitize exactly as before, so existing
        // Ingress child names (hanzo-domains-*) never churn.
        assert_eq!(sanitize_label("api.cloud.hanzo.ai"), "api-cloud-hanzo-ai");
    }
}

pub async fn run_ingress_controller(client: Client, namespace: String, api_group: String) {
    let api: Api<IngressCR> = if namespace.is_empty() {
        Api::all(client.clone())
    } else {
        Api::namespaced(client.clone(), &namespace)
    };
    info!(group = %api_group, "Starting Ingress controller");
    let ctx = Arc::new(Ctx { client, api_group });
    Controller::new(api, Config::default())
        .run(reconcile, on_error, ctx)
        .for_each(|res| async move {
            if let Err(e) = res {
                warn!(error = %e, "Ingress reconcile error");
            }
        })
        .await;
}
