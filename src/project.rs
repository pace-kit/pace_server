use serde::{Deserialize, Serialize};

use crate::error::ApiError;

/// Limit applies to the complete uncompressed project, including whitespace.
pub const MAX_PROJECT_BYTES: usize = 1024 * 1024;

/// Transport validation only: the upstream core does not yet define a scene schema.
/// RawValue validates JSON without interpreting or normalizing numbers/extension fields.
pub fn validate_project(bytes: &[u8]) -> Result<(), ApiError> {
    if bytes.len() > MAX_PROJECT_BYTES {
        return Err(ApiError::too_large());
    }
    serde_json::from_slice::<&serde_json::value::RawValue>(bytes)
        .map_err(|_| ApiError::bad_request("invalid_project", "Project must be valid JSON"))?;
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Reader,
    Writer,
}

impl Role {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Reader => "reader",
            Self::Writer => "writer",
        }
    }
}

pub fn etag(revision: i64) -> String {
    format!("\"{revision}\"")
}

/// Require one explicit, strong revision; wildcard writes would defeat concurrency control.
pub fn parse_revision(value: &str) -> Result<i64, ApiError> {
    let value = value.trim();
    let revision = value.strip_prefix('"').and_then(|v| v.strip_suffix('"'));
    if let Some(value) = revision {
        if let Ok(number) = value.parse::<i64>() {
            if number > 0 && number.to_string() == value {
                return Ok(number);
            }
        }
    }
    Err(ApiError::bad_request(
        "invalid_revision",
        "If-Match must contain one strong revision ETag, for example \"1\"",
    ))
}
