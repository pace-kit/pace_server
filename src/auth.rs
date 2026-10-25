use std::{collections::HashSet, sync::Arc};

use axum::{
    extract::{Request, State},
    http::{header, HeaderValue},
    middleware::{self, Next},
    response::Response,
    routing::get,
    Router,
};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

use crate::{
    error::ApiError,
    storage::{Principal, Store},
};

/// Provisioned bearer credentials; only SHA-256 digests are retained.
/// Tokens must be random secrets, not passwords (no password hashing/login API).
#[derive(Clone)]
pub struct Auth(Arc<Vec<Credential>>);

struct Credential {
    principal: String,
    digest: [u8; 32],
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AuthFile {
    tokens: Vec<Token>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Token {
    principal: String,
    sha256: String,
}

pub(crate) fn valid_principal(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.@".contains(&b))
}

impl Auth {
    pub fn from_json(bytes: &[u8]) -> Result<Self, &'static str> {
        let config: AuthFile =
            serde_json::from_slice(bytes).map_err(|_| "Invalid authentication config JSON")?;
        if config.tokens.is_empty() || config.tokens.len() > 1024 {
            return Err("Authentication config requires 1 to 1024 token digests");
        }
        let mut seen = HashSet::new();
        let mut credentials = Vec::new();
        for token in config.tokens {
            if !valid_principal(&token.principal) {
                return Err("Invalid authentication principal identifier");
            }
            if token.sha256.len() != 64 || !token.sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err("Token digest must contain exactly 64 hexadecimal characters");
            }
            let mut digest = [0; 32];
            for (i, byte) in digest.iter_mut().enumerate() {
                *byte = u8::from_str_radix(&token.sha256[2 * i..2 * i + 2], 16)
                    .map_err(|_| "Invalid token digest")?;
            }
            if !seen.insert(digest) {
                return Err("Duplicate token digest");
            }
            credentials.push(Credential {
                principal: token.principal,
                digest,
            });
        }
        Ok(Self(Arc::new(credentials)))
    }

    fn authenticate(&self, token: &str) -> Option<Principal> {
        if !(32..=512).contains(&token.len())
            || !token
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-._~+/=".contains(&b))
        {
            return None;
        }
        let digest: [u8; 32] = Sha256::digest(token.as_bytes()).into();
        let mut principal = None;
        // Compare all fixed-size digests; no plaintext tokens or prefix comparison.
        for credential in self.0.iter() {
            if bool::from(credential.digest.ct_eq(&digest)) {
                principal = Some(Principal(credential.principal.clone()));
            }
        }
        principal
    }
}

/// Authentication runs before any project body is collected. Only liveness is public.
pub fn authenticated_router(store: Store, auth: Auth) -> Router {
    crate::api::project_routes(store)
        .route_layer(middleware::from_fn_with_state(auth, authenticate))
        .route("/health", get(|| async { "ok" }))
}

async fn authenticate(
    State(auth): State<Auth>,
    mut request: Request,
    next: Next,
) -> Result<Response, ApiError> {
    let mut values = request.headers().get_all(header::AUTHORIZATION).iter();
    let value = values
        .next()
        .and_then(|v| v.to_str().ok())
        .ok_or_else(ApiError::unauthorized)?;
    if values.next().is_some() {
        return Err(ApiError::unauthorized());
    }
    let (scheme, token) = value.split_once(' ').ok_or_else(ApiError::unauthorized)?;
    if !scheme.eq_ignore_ascii_case("Bearer") {
        return Err(ApiError::unauthorized());
    }
    let principal = auth
        .authenticate(token.trim_start_matches(' '))
        .ok_or_else(ApiError::unauthorized)?;
    request.extensions_mut().insert(principal);
    let mut response = next.run(request).await;
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(response)
}
