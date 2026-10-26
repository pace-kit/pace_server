use serde::{Deserialize, Serialize};

use crate::error::ApiError;

/// Limit applies to the complete uncompressed project, including whitespace.
pub const MAX_PROJECT_BYTES: usize = pace_editor_format::MAX_PROJECT_BYTES;

/// Validate with the pinned upstream loaders, without reserializing stored bytes.
/// Drafts remain editable (including unresolved references); core scenes must be runnable.
pub fn validate_project(bytes: &[u8]) -> Result<(), ApiError> {
    if bytes.len() > MAX_PROJECT_BYTES {
        return Err(ApiError::too_large());
    }
    let text = std::str::from_utf8(bytes)
        .map_err(|_| ApiError::bad_request("invalid_project", "Project must be UTF-8 JSON"))?;
    #[derive(Deserialize)]
    struct Format {
        format: Option<String>,
        format_version: Option<u32>,
    }
    let header: Format = serde_json::from_str(text).map_err(|_| {
        ApiError::bad_request(
            "invalid_project",
            "Project must be a supported JSON document",
        )
    })?;
    match (header.format.as_deref(), header.format_version) {
        (Some(pace_editor_format::FORMAT), None) => {
            pace_editor_format::Project::from_json(text).map_err(|_| {
                ApiError::bad_request(
                    "invalid_project",
                    "Invalid editor draft: check its version, structure and object IDs",
                )
            })?;
        }
        (None, Some(_)) => {
            pace_core::scene::Scene::from_json(text).map_err(|_| {
                ApiError::bad_request(
                    "invalid_project",
                    "Invalid core scene: check its version, structure, IDs, references and bounds",
                )
            })?;
        }
        _ => {
            return Err(ApiError::bad_request(
                "unsupported_format",
                "Expected a pace-editor-draft v1 or PACE core scene v1",
            ))
        }
    }
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
