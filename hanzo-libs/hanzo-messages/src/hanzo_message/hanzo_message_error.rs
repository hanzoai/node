use std::fmt;

#[derive(Debug)]
pub enum HanzoMessageError {
    SigningError(String),
    DecryptionError(String),
    EncryptionError(String),
    InvalidMessageSchemaType(String),
    MissingMessageBody(String),
    DeserializationError(String),
    SerializationError(String),
    AlreadyEncrypted(String),
}

impl fmt::Display for HanzoMessageError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            HanzoMessageError::SigningError(msg) => write!(f, "SigningError: {}", msg),
            HanzoMessageError::DecryptionError(msg) => write!(f, "DecryptionError: {}", msg),
            HanzoMessageError::EncryptionError(msg) => write!(f, "EncryptionError: {}", msg),
            HanzoMessageError::InvalidMessageSchemaType(msg) => write!(f, "InvalidMessageSchemaType: {}", msg),
            HanzoMessageError::MissingMessageBody(msg) => write!(f, "MissingMessageBody: {}", msg),
            HanzoMessageError::DeserializationError(msg) => write!(f, "DeserializationError: {}", msg),
            HanzoMessageError::SerializationError(msg) => write!(f, "SerializationError: {}", msg),
            HanzoMessageError::AlreadyEncrypted(msg) => write!(f, "AlreadyEncrypted: {}", msg),
        }
    }
}

impl std::error::Error for HanzoMessageError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        // Note: Update this if we wrap other error and we want to return the source (underlying cause).
        None
    }
}
