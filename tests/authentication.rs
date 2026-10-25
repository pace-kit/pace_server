mod common;

use common::{client, Server, ALICE, BOB, EVE, PROJECT};
use reqwest::Client;
use tempfile::tempdir;

#[tokio::test]
async fn every_project_operation_requires_authentication() {
    let dir = tempdir().unwrap();
    let server = Server::start(&dir.path().join("projects.db")).await;
    let (url, _) = server.project().await;
    let guest = Client::new();
    for token in [
        None,
        Some("Bearer invalid"),
        Some("Basic invalid"),
        Some("Bearer aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
        Some("Bearer"),
        Some("Bearer "),
    ] {
        for (method, url) in [
            ("POST", format!("{}/v1/projects", server.base)),
            ("GET", url.clone()),
            ("PUT", url.clone()),
            ("PUT", format!("{url}/members/bob")),
            ("DELETE", format!("{url}/members/bob")),
        ] {
            let mut request = guest
                .request(method.parse().unwrap(), url)
                .header("Content-Type", "application/json")
                .header("If-Match", "\"1\"")
                .body(PROJECT);
            if let Some(token) = token {
                request = request.header("Authorization", token);
            }
            let response = request.send().await.unwrap();
            assert_eq!(response.status(), 401, "{method} {token:?}");
            assert!(response.headers()["WWW-Authenticate"]
                .to_str()
                .unwrap()
                .starts_with("Bearer"));
            assert_eq!(response.headers()["Cache-Control"], "no-store");
        }
    }
    assert_eq!(
        guest
            .get(format!("{}/health", server.base))
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    // Reject ambiguous headers; auth scheme itself is case insensitive.
    assert_eq!(
        guest
            .get(&url)
            .header("Authorization", format!("Bearer {ALICE}"))
            .header("Authorization", format!("Bearer {BOB}"))
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    assert_eq!(
        guest
            .get(url)
            .header("Authorization", format!("bearer {ALICE}"))
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
}

#[tokio::test]
async fn owner_grants_reader_writer_access_and_revokes_it() {
    let dir = tempdir().unwrap();
    let server = Server::start(&dir.path().join("projects.db")).await;
    let (url, _) = server.project().await;
    let bob = client(BOB);
    let eve = client(EVE);
    let member = format!("{url}/members/bob");

    assert_eq!(bob.get(&url).send().await.unwrap().status(), 404);
    assert_eq!(
        bob.put(&url)
            .header("If-Match", "\"999\"")
            .header("Content-Type", "application/json")
            .body(PROJECT)
            .send()
            .await
            .unwrap()
            .status(),
        404
    );
    assert_eq!(
        bob.put(&member)
            .json(&serde_json::json!({"role":"writer"}))
            .send()
            .await
            .unwrap()
            .status(),
        404
    );
    assert_eq!(
        server
            .client
            .put(&member)
            .json(&serde_json::json!({"role":"reader"}))
            .send()
            .await
            .unwrap()
            .status(),
        204
    );
    let downloaded = bob.get(&url).send().await.unwrap();
    assert_eq!(downloaded.status(), 200);
    assert_eq!(downloaded.bytes().await.unwrap().as_ref(), PROJECT);
    let denied = bob
        .put(&url)
        .header("If-Match", "\"999\"")
        .header("Content-Type", "application/json")
        .body(PROJECT)
        .send()
        .await
        .unwrap();
    assert_eq!(denied.status(), 403);
    assert!(denied
        .json::<serde_json::Value>()
        .await
        .unwrap()
        .get("current_revision")
        .is_none());

    assert_eq!(
        server
            .client
            .put(&member)
            .json(&serde_json::json!({"role":"writer"}))
            .send()
            .await
            .unwrap()
            .status(),
        204
    );
    let edit = b"{\"edited_by\":\"bob\",\"scene\":{}}";
    let response = bob
        .put(&url)
        .header("If-Match", "\"1\"")
        .header("Content-Type", "application/json")
        .body(edit.to_vec())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 204);
    assert_eq!(response.headers()["ETag"], "\"2\"");
    assert_eq!(
        server
            .client
            .get(&url)
            .send()
            .await
            .unwrap()
            .bytes()
            .await
            .unwrap()
            .as_ref(),
        edit
    );
    assert_eq!(eve.get(&url).send().await.unwrap().status(), 404);
    // A writer cannot change permissions or transfer ownership by changing JSON.
    assert_eq!(
        bob.put(format!("{url}/members/eve"))
            .json(&serde_json::json!({"role":"writer"}))
            .send()
            .await
            .unwrap()
            .status(),
        404
    );
    assert_eq!(bob.delete(&member).send().await.unwrap().status(), 404);
    assert_eq!(
        server
            .client
            .delete(format!("{url}/members/alice"))
            .send()
            .await
            .unwrap()
            .status(),
        400
    );
    assert_eq!(
        server
            .client
            .put(&member)
            .json(&serde_json::json!({"role":"owner"}))
            .send()
            .await
            .unwrap()
            .status(),
        400
    );
    assert_eq!(
        server.client.delete(&member).send().await.unwrap().status(),
        204
    );
    assert_eq!(bob.get(&url).send().await.unwrap().status(), 404);
    assert_eq!(
        bob.put(&url)
            .header("If-Match", "\"2\"")
            .header("Content-Type", "application/json")
            .body(PROJECT)
            .send()
            .await
            .unwrap()
            .status(),
        404
    );
    assert_eq!(
        server.client.get(url).send().await.unwrap().headers()["ETag"],
        "\"2\""
    );
}

#[tokio::test]
async fn each_creator_owns_only_their_own_projects() {
    let dir = tempdir().unwrap();
    let server = Server::start(&dir.path().join("projects.db")).await;
    let bob = client(BOB);
    let created = bob
        .post(format!("{}/v1/projects", server.base))
        .header("Content-Type", "application/json")
        .body("{\"owner\":\"alice\"}")
        .send()
        .await
        .unwrap();
    assert_eq!(created.status(), 201);
    let url = format!(
        "{}{}",
        server.base,
        created.headers()["Location"].to_str().unwrap()
    );
    assert_eq!(bob.get(&url).send().await.unwrap().status(), 200);
    assert_eq!(server.client.get(&url).send().await.unwrap().status(), 404);
    assert_eq!(
        server
            .client
            .put(format!("{url}/members/alice"))
            .json(&serde_json::json!({"role":"writer"}))
            .send()
            .await
            .unwrap()
            .status(),
        404
    );
}

#[tokio::test]
async fn missing_auth_is_rejected_before_waiting_for_an_upload_body() {
    use std::{
        io::{Read, Write},
        net::TcpStream,
        time::Duration,
    };
    let dir = tempdir().unwrap();
    let server = Server::start(&dir.path().join("projects.db")).await;
    let address = server.base.trim_start_matches("http://").to_owned();
    tokio::task::spawn_blocking(move || {
        let mut socket = TcpStream::connect(&address).unwrap();
        socket.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
        write!(socket, "POST /v1/projects HTTP/1.1\r\nHost: {address}\r\nContent-Type: application/json\r\nContent-Length: 100000\r\nConnection: close\r\n\r\n").unwrap();
        let mut response = [0; 1024];
        let size = socket.read(&mut response).unwrap();
        assert!(String::from_utf8_lossy(&response[..size]).starts_with("HTTP/1.1 401"));
    }).await.unwrap();
}

#[tokio::test]
async fn chunked_uploads_cannot_bypass_the_size_limit() {
    use std::{
        io::{Read, Write},
        net::TcpStream,
        time::Duration,
    };
    let dir = tempdir().unwrap();
    let server = Server::start(&dir.path().join("projects.db")).await;
    let address = server.base.trim_start_matches("http://").to_owned();
    tokio::task::spawn_blocking(move || {
        let mut socket = TcpStream::connect(&address).unwrap();
        socket.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        socket.set_write_timeout(Some(Duration::from_secs(5))).unwrap();
        write!(socket, "POST /v1/projects HTTP/1.1\r\nHost: {address}\r\nAuthorization: Bearer {ALICE}\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n").unwrap();
        let mut chunk = b"4000\r\n".to_vec();
        chunk.extend(vec![b' '; 16 * 1024]);
        chunk.extend_from_slice(b"\r\n");
        for _ in 0..65 { if socket.write_all(&chunk).is_err() { break; } }
        let _ = socket.write_all(b"2\r\n{}\r\n0\r\n\r\n");
        let mut response = [0; 2048];
        let size = socket.read(&mut response).unwrap();
        assert!(String::from_utf8_lossy(&response[..size]).starts_with("HTTP/1.1 413"));
    }).await.unwrap();
}

#[test]
fn auth_configuration_is_strict_and_supports_token_rotation() {
    use pace_server::Auth;
    use sha2::{Digest, Sha256};
    assert!(Auth::from_json(&common::auth_json()).is_ok());
    for invalid in [
        "{}",
        "{\"tokens\":[]}",
        "{\"tokens\":[{\"principal\":\"a\",\"sha256\":\"x\"}]}",
    ] {
        assert!(Auth::from_json(invalid.as_bytes()).is_err());
    }
    let hash = format!("{:x}", Sha256::digest(ALICE.as_bytes()));
    let invalid = serde_json::json!({"tokens":[{"principal":"../alice", "sha256":hash}]});
    assert!(Auth::from_json(&serde_json::to_vec(&invalid).unwrap()).is_err());
    let duplicate = serde_json::json!({"tokens":[{"principal":"alice", "sha256":hash},{"principal":"bob", "sha256":hash}]});
    assert!(Auth::from_json(&serde_json::to_vec(&duplicate).unwrap()).is_err());
    let rotated = serde_json::json!({"tokens":[{"principal":"alice", "sha256":hash},
        {"principal":"alice", "sha256":format!("{:x}", Sha256::digest(BOB.as_bytes()))}]});
    assert!(Auth::from_json(&serde_json::to_vec(&rotated).unwrap()).is_ok());
}
