pub mod did;
pub mod document;
pub mod error;
pub mod resolver;
pub mod verification_method;
pub mod service;
pub mod proof;

pub use did::{DID, Network};
pub use document::DIDDocument;
pub use error::DIDError;
pub use resolver::DIDResolver;
pub use verification_method::{VerificationMethod, VerificationMethodType};
pub use service::{Service, ServiceEndpoint};
pub use proof::Proof;

// Re-export commonly used types
pub use serde_json::Value as JsonValue;