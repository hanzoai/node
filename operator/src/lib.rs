//! Hanzo Operator library entrypoint.

// Exported APIs (core::iam_admin, secret, status helpers) intentionally
// outpace current call sites so external integration tests + future
// reconcilers can consume them. Suppress the dead-code lint at crate root
// rather than scatter `#[allow]` on each item.
#![allow(dead_code)]

pub mod api_group;
pub mod apply;
pub mod controllers;
pub mod core;
pub mod crd;
pub mod crd_bundle;
pub mod crd_types;
pub mod install;
pub mod manifests;
pub mod run;
pub mod zapclient;
