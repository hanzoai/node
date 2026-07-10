//! CRD YAML bundle generator.
//!
//! Emits one multi-document YAML stream carrying every CRD this operator
//! manages, at the requested API group. The compile-time group baked into the
//! `#[derive(CustomResource)]` macros is `hanzo.ai`; the shared
//! [`operator::crd_bundle::bundle`] rewrites `spec.group` (and the derived
//! `metadata.name = <plural>.<group>`) so the same Rust types produce the
//! per-universe bundles checked in under `k8s/crds/all-<group>.yaml`.
//!
//! Usage:
//!
//! ```text
//! generate-crd-yaml [--api-group <group>]
//! ```
//!
//! `--api-group` (or `OPERATOR_API_GROUP`) defaults to `hanzo.ai`. Other
//! universes: `lux.cloud`, `zoo.cloud`, `osage.cloud`. The Kind set + ordering
//! is canonical and lives in `crd_bundle` (shared with the `install` path).

use operator::api_group::ApiGroup;
use operator::crd_bundle::bundle;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Tiny hand-rolled flag parse — this binary takes exactly one optional
    // flag and pulling `clap` into a generator is unwarranted weight.
    let mut api_group: Option<String> = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--api-group" => {
                api_group = Some(args.next().ok_or("--api-group requires a value")?);
            }
            other if other.starts_with("--api-group=") => {
                api_group = Some(other["--api-group=".len()..].to_string());
            }
            "-h" | "--help" => {
                eprintln!("usage: generate-crd-yaml [--api-group <group>]");
                return Ok(());
            }
            other => return Err(format!("unknown argument: {other}").into()),
        }
    }

    let group = ApiGroup::resolve(api_group.as_deref()).group;
    let mut out = String::new();
    for crd in bundle(&group) {
        out.push_str("---\n");
        out.push_str(&serde_yaml::to_string(&crd)?);
    }
    print!("{out}");
    Ok(())
}
