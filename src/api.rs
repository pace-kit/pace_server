use axum::{
    body::Bytes,
    extract::{rejection::BytesRejection, DefaultBodyLimit, Path, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post, put},
    Extension, Json, Router,
};
use serde::{Deserialize, Serialize};

use crate::{
    error::ApiError,
    project::{etag, parse_revision, Role, MAX_PROJECT_BYTES},
    storage::{Principal, Store},
};

/// Without a trusted layer inserting Principal, all project routes fail closed.
pub fn router(store: Store) -> Router {
    Router::new()
        .route("/health", get(|| async { "ok" }))
        .route("/v1/projects", post(create))
        .route("/v1/projects/{id}", get(download).put(upload))
        .route(
            "/v1/projects/{id}/members/{member}",
            put(grant).delete(revoke),
        )
        .layer(DefaultBodyLimit::max(MAX_PROJECT_BYTES))
        .with_state(store)
}

fn principal(value: Option<Extension<Principal>>) -> Result<Principal, ApiError> {
    value.map(|v| v.0).ok_or_else(ApiError::unauthorized)
}

fn project_id(value: String) -> Result<String, ApiError> {
    // Canonical UUIDs are identifiers, never filesystem paths.
    uuid::Uuid::parse_str(&value).map_err(|_| ApiError::not_found())?;
    Ok(value)
}

fn body(headers: &HeaderMap, bytes: Result<Bytes, BytesRejection>) -> Result<Vec<u8>, ApiError> {
    if headers.contains_key(header::CONTENT_ENCODING)
        || headers
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.split(';').next())
            .is_none_or(|v| !v.trim().eq_ignore_ascii_case("application/json"))
    {
        return Err(ApiError::new(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "unsupported_media_type",
            "Send uncompressed application/json",
        ));
    }
    bytes.map(|b| b.to_vec()).map_err(|error| {
        if error.status() == StatusCode::PAYLOAD_TOO_LARGE {
            ApiError::too_large()
        } else {
            ApiError::bad_request("invalid_body", "Could not read request body")
        }
    })
}

fn versioned(status: StatusCode, revision: i64) -> Response {
    let mut response = status.into_response();
    response.headers_mut().insert(
        header::ETAG,
        HeaderValue::from_str(&etag(revision)).unwrap(),
    );
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

#[derive(Serialize)]
struct Created {
    id: String,
    revision: i64,
}

async fn create(
    State(store): State<Store>,
    identity: Option<Extension<Principal>>,
    headers: HeaderMap,
    bytes: Result<Bytes, BytesRejection>,
) -> Result<Response, ApiError> {
    let snapshot = store
        .create(principal(identity)?, body(&headers, bytes)?)
        .await?;
    let mut response = versioned(StatusCode::CREATED, snapshot.revision);
    response.headers_mut().insert(
        header::LOCATION,
        HeaderValue::from_str(&format!("/v1/projects/{}", snapshot.id)).unwrap(),
    );
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    *response.body_mut() = serde_json::to_vec(&Created {
        id: snapshot.id,
        revision: snapshot.revision,
    })
    .unwrap()
    .into();
    Ok(response)
}

async fn download(
    State(store): State<Store>,
    identity: Option<Extension<Principal>>,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    let snapshot = store.get(principal(identity)?, project_id(id)?).await?;
    let mut response = versioned(StatusCode::OK, snapshot.revision);
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    *response.body_mut() = snapshot.body.into();
    Ok(response)
}

async fn upload(
    State(store): State<Store>,
    identity: Option<Extension<Principal>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    bytes: Result<Bytes, BytesRejection>,
) -> Result<Response, ApiError> {
    let identity = principal(identity)?;
    let id = project_id(id)?;
    let values: Vec<_> = headers.get_all(header::IF_MATCH).iter().collect();
    if values.is_empty() {
        return Err(ApiError::new(
            StatusCode::PRECONDITION_REQUIRED,
            "revision_required",
            "Send the downloaded ETag in If-Match",
        ));
    }
    if values.len() != 1 {
        return Err(ApiError::bad_request(
            "invalid_revision",
            "Send exactly one If-Match header",
        ));
    }
    let expected = parse_revision(
        values[0]
            .to_str()
            .map_err(|_| ApiError::bad_request("invalid_revision", "Invalid If-Match header"))?,
    )?;
    let revision = store
        .update(identity, id, expected, body(&headers, bytes)?)
        .await?;
    Ok(versioned(StatusCode::NO_CONTENT, revision))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Member {
    role: Role,
}

fn member_id(member: String) -> Result<String, ApiError> {
    if member.is_empty()
        || member.len() > 128
        || !member
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.@".contains(&b))
    {
        return Err(ApiError::bad_request(
            "invalid_member",
            "Invalid principal identifier",
        ));
    }
    Ok(member)
}

async fn grant(
    State(store): State<Store>,
    identity: Option<Extension<Principal>>,
    Path((id, member)): Path<(String, String)>,
    payload: Result<Json<Member>, axum::extract::rejection::JsonRejection>,
) -> Result<StatusCode, ApiError> {
    let identity = principal(identity)?;
    let Json(payload) = payload
        .map_err(|_| ApiError::bad_request("invalid_member", "Expected a reader or writer role"))?;
    store
        .set_member(
            identity,
            project_id(id)?,
            member_id(member)?,
            Some(payload.role),
        )
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn revoke(
    State(store): State<Store>,
    identity: Option<Extension<Principal>>,
    Path((id, member)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    store
        .set_member(
            principal(identity)?,
            project_id(id)?,
            member_id(member)?,
            None,
        )
        .await?;
    Ok(StatusCode::NO_CONTENT)
}
