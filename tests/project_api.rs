mod common;

use common::{Server, PROJECT};
use pace_server::project::MAX_PROJECT_BYTES;
use reqwest::Client;
use tempfile::tempdir;

#[tokio::test]
async fn two_clients_exchange_exact_bytes_and_stale_writes_conflict() {
    let dir = tempdir().unwrap();
    let server = Server::start(&dir.path().join("projects.db")).await;
    let (url, revision) = server.project().await;
    let second_client = common::client(common::ALICE);
    let response = second_client.get(&url).send().await.unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()["ETag"], revision);
    assert_eq!(response.bytes().await.unwrap().as_ref(), PROJECT);

    let next = b"{\"scene\":{\"nodes\":[{\"name\":\"edited\"}]},\"unknown\":true}";
    let response = second_client
        .put(&url)
        .header("If-Match", &revision)
        .header("Content-Type", "application/json")
        .body(next.to_vec())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 204);
    assert_eq!(response.headers()["ETag"], "\"2\"");
    let stale = server
        .client
        .put(&url)
        .header("If-Match", revision)
        .header("Content-Type", "application/json")
        .body(PROJECT)
        .send()
        .await
        .unwrap();
    assert_eq!(stale.status(), 412);
    let error: serde_json::Value = stale.json().await.unwrap();
    assert_eq!(error["error"], "revision_conflict");
    assert_eq!(error["current_revision"], 2);
    let response = server.client.get(url).send().await.unwrap();
    assert_eq!(response.bytes().await.unwrap().as_ref(), next);
}

#[tokio::test]
async fn writes_require_one_explicit_strong_revision() {
    let dir = tempdir().unwrap();
    let server = Server::start(&dir.path().join("projects.db")).await;
    let (url, _) = server.project().await;
    for (value, status) in [
        (None, 428),
        (Some("*"), 400),
        (Some("W/\"1\""), 400),
        (Some("\"1\", \"2\""), 400),
        (Some("1"), 400),
        (Some("\"01\""), 400),
        (Some("\"0\""), 400),
        (Some("\"-1\""), 400),
        (Some("\"9999999999999999999999\""), 400),
        (Some("\"99\""), 412),
    ] {
        let mut request = server
            .client
            .put(&url)
            .header("Content-Type", "application/json")
            .body(PROJECT);
        if let Some(value) = value {
            request = request.header("If-Match", value);
        }
        assert_eq!(request.send().await.unwrap().status(), status, "{value:?}");
    }
    let response = server
        .client
        .put(&url)
        .header("Content-Type", "application/json")
        .header("If-Match", "\"1\"")
        .header("If-Match", "\"1\"")
        .body(PROJECT)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 400);
    assert_eq!(
        server.client.get(url).send().await.unwrap().headers()["ETag"],
        "\"1\""
    );
}

#[tokio::test]
async fn independent_database_connections_cannot_both_win_a_revision() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("projects.db");
    let first = Server::start(&path).await;
    let second = Server::start(&path).await;
    let (url, revision) = first.project().await;
    let second_url = url.replace(&first.base, &second.base);
    let a = first
        .client
        .put(&url)
        .header("If-Match", &revision)
        .header("Content-Type", "application/json")
        .body("{\"winner\":1}")
        .send();
    let b = second
        .client
        .put(&second_url)
        .header("If-Match", &revision)
        .header("Content-Type", "application/json")
        .body("{\"winner\":2}")
        .send();
    let (a, b) = tokio::join!(a, b);
    let a = a.unwrap().status();
    let b = b.unwrap().status();
    assert!((a == 204 && b == 412) || (a == 412 && b == 204));
    assert_eq!(
        first.client.get(url).send().await.unwrap().headers()["ETag"],
        "\"2\""
    );
}

#[tokio::test]
async fn snapshots_and_revisions_survive_reopening_the_database() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("projects.db");
    let server = Server::start(&path).await;
    let (url, revision) = server.project().await;
    let location = url.strip_prefix(&server.base).unwrap().to_owned();
    let response = server
        .client
        .put(&url)
        .header("If-Match", revision)
        .header("Content-Type", "application/json")
        .body(PROJECT)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 204);
    drop(server);
    let server = Server::start(&path).await;
    let response = server
        .client
        .get(format!("{}{location}", server.base))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()["ETag"], "\"2\"");
    assert_eq!(response.bytes().await.unwrap().as_ref(), PROJECT);
}

#[tokio::test]
async fn malformed_and_oversized_uploads_leave_existing_project_unchanged() {
    let dir = tempdir().unwrap();
    let server = Server::start(&dir.path().join("projects.db")).await;
    let (url, revision) = server.project().await;
    let mut oversized = vec![b' '; MAX_PROJECT_BYTES];
    oversized.extend_from_slice(b"{}");
    for (bytes, status) in [
        (b"{".to_vec(), 400),
        (vec![], 400),
        (b"{} {}".to_vec(), 400),
        (vec![0xff], 400),
        (b"{\"n\":NaN}".to_vec(), 400),
        (oversized, 413),
    ] {
        assert_eq!(server.create(bytes.clone()).await.status(), status);
        let response = server
            .client
            .put(&url)
            .header("If-Match", &revision)
            .header("Content-Type", "application/json")
            .body(bytes)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), status);
    }
    let response = server.client.get(url).send().await.unwrap();
    assert_eq!(response.headers()["ETag"], revision);
    assert_eq!(response.bytes().await.unwrap().as_ref(), PROJECT);
}

#[tokio::test]
async fn exact_size_limit_is_accepted_and_unsupported_media_is_rejected() {
    let dir = tempdir().unwrap();
    let server = Server::start(&dir.path().join("projects.db")).await;
    let mut body = b"{}".to_vec();
    body.resize(MAX_PROJECT_BYTES, b' ');
    assert_eq!(server.create(body).await.status(), 201);
    let url = format!("{}/v1/projects", server.base);
    assert_eq!(
        server
            .client
            .post(&url)
            .body(PROJECT)
            .send()
            .await
            .unwrap()
            .status(),
        415
    );
    assert_eq!(
        server
            .client
            .post(url)
            .header("Content-Type", "application/json")
            .header("Content-Encoding", "gzip")
            .body(PROJECT)
            .send()
            .await
            .unwrap()
            .status(),
        415
    );
}

#[tokio::test]
async fn missing_projects_are_not_implicitly_created_by_put() {
    let dir = tempdir().unwrap();
    let server = Server::start(&dir.path().join("projects.db")).await;
    let url = format!("{}/v1/projects/{}", server.base, uuid::Uuid::new_v4());
    assert_eq!(server.client.get(&url).send().await.unwrap().status(), 404);
    assert_eq!(
        server
            .client
            .put(&url)
            .header("If-Match", "\"1\"")
            .header("Content-Type", "application/json")
            .body(PROJECT)
            .send()
            .await
            .unwrap()
            .status(),
        404
    );
}

#[tokio::test]
async fn router_fails_closed_without_a_trusted_identity_layer() {
    let dir = tempdir().unwrap();
    let router =
        pace_server::router(pace_server::Store::open(dir.path().join("projects.db")).unwrap());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/v1/projects", listener.local_addr().unwrap());
    let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let response = Client::new()
        .post(url)
        .header("Content-Type", "application/json")
        .body(PROJECT)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 401);
    task.abort();
}
