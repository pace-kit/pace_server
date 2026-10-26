mod common;

use common::{Server, PROJECT};
use tempfile::tempdir;

const DRAFT: &[u8] = include_bytes!("fixtures/editor-draft-v1.json");

#[tokio::test]
async fn both_upstream_formats_roundtrip_with_original_bytes_and_assets() {
    let dir = tempdir().unwrap();
    let server = Server::start(&dir.path().join("projects.db")).await;
    for fixture in [PROJECT, DRAFT] {
        let response = server.create(fixture).await;
        assert_eq!(response.status(), 201);
        let url = format!(
            "{}{}",
            server.base,
            response.headers()["Location"].to_str().unwrap()
        );
        let downloaded = server
            .client
            .get(url)
            .send()
            .await
            .unwrap()
            .bytes()
            .await
            .unwrap();
        assert_eq!(downloaded.as_ref(), fixture);
        let text = std::str::from_utf8(&downloaded).unwrap();
        if fixture == PROJECT {
            pace_core::scene::Scene::from_json(text).unwrap();
        } else {
            assert!(pace_editor_format::Project::from_json(text)
                .unwrap()
                .validate()
                .is_empty());
        }
    }
}

#[tokio::test]
async fn schema_errors_and_future_versions_never_replace_existing_content() {
    let dir = tempdir().unwrap();
    let server = Server::start(&dir.path().join("projects.db")).await;
    let (url, etag) = server.project().await;
    let core: serde_json::Value = serde_json::from_slice(PROJECT).unwrap();
    let draft: serde_json::Value = serde_json::from_slice(DRAFT).unwrap();
    let mut invalid = vec![
        serde_json::json!({}),
        serde_json::json!([]),
        serde_json::json!(null),
    ];
    for (pointer, value) in [
        ("/format_version", serde_json::json!(99)),
        ("/start_room", serde_json::json!("missing")),
        ("/rooms/0/hotspots/0/rect", serde_json::json!([2, 0, 1, 1])),
        ("/rooms/1/id", serde_json::json!("quay")),
    ] {
        let mut scene = core.clone();
        *scene.pointer_mut(pointer).unwrap() = value;
        invalid.push(scene);
    }
    let mut unknown = core.clone();
    unknown["typo"] = true.into();
    invalid.push(unknown);
    for (pointer, value) in [
        ("/version", serde_json::json!(99)),
        ("/rooms/1/id", draft["rooms"][0]["id"].clone()),
        (
            "/rooms/0/hotspots/0/action/kind",
            serde_json::json!("run_script"),
        ),
    ] {
        let mut scene = draft.clone();
        *scene.pointer_mut(pointer).unwrap() = value;
        invalid.push(scene);
    }
    let mut unknown = draft.clone();
    unknown["owner"] = "eve".into();
    invalid.push(unknown);
    for value in invalid {
        let bytes = serde_json::to_vec(&value).unwrap();
        assert_eq!(server.create(bytes.clone()).await.status(), 400, "{value}");
        assert_eq!(
            server
                .client
                .put(&url)
                .header("If-Match", &etag)
                .header("Content-Type", "application/json")
                .body(bytes)
                .send()
                .await
                .unwrap()
                .status(),
            400
        );
    }
    let downloaded = server.client.get(url).send().await.unwrap();
    assert_eq!(downloaded.headers()["ETag"], etag);
    assert_eq!(downloaded.bytes().await.unwrap().as_ref(), PROJECT);
}

#[tokio::test]
async fn editable_drafts_preserve_incomplete_references_like_local_save_load() {
    let dir = tempdir().unwrap();
    let server = Server::start(&dir.path().join("projects.db")).await;
    let mut project =
        pace_editor_format::Project::from_json(std::str::from_utf8(DRAFT).unwrap()).unwrap();
    project.start_room = Some("unfinished-room".into());
    let bytes = project.to_json().unwrap().into_bytes();
    assert!(!project.validate().is_empty());
    assert_eq!(server.create(bytes).await.status(), 201);
}

#[tokio::test]
async fn only_configured_origins_can_read_responses_and_revision_headers() {
    let dir = tempdir().unwrap();
    let server = Server::start(&dir.path().join("projects.db")).await;
    let client = reqwest::Client::new();
    let url = format!("{}/v1/projects", server.base);
    for origin in ["https://editor.example", "https://untrusted.example"] {
        let response = client
            .request(reqwest::Method::OPTIONS, &url)
            .header("Origin", origin)
            .header("Access-Control-Request-Method", "PUT")
            .header(
                "Access-Control-Request-Headers",
                "authorization,content-type,if-match",
            )
            .send()
            .await
            .unwrap();
        assert!(response.status().is_success());
        if origin == "https://editor.example" {
            assert_eq!(response.headers()["Access-Control-Allow-Origin"], origin);
            assert!(response.headers()["Access-Control-Allow-Headers"]
                .to_str()
                .unwrap()
                .contains("if-match"));
        } else {
            assert!(response
                .headers()
                .get("Access-Control-Allow-Origin")
                .is_none());
        }
    }
    let response = server
        .client
        .post(&url)
        .header("Origin", "https://editor.example")
        .header("Content-Type", "application/json")
        .body(PROJECT)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 201);
    assert_eq!(
        response.headers()["Access-Control-Allow-Origin"],
        "https://editor.example"
    );
    assert!(response.headers()["Access-Control-Expose-Headers"]
        .to_str()
        .unwrap()
        .contains("etag"));
    assert_eq!(
        client
            .post(url)
            .header("Origin", "https://editor.example")
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    for bad in [
        "*",
        "null",
        "https://editor.example/path",
        "https://user:pass@editor.example",
        "ftp://editor.example",
    ] {
        assert!(pace_server::cors::cors(bad).is_err());
    }
}
