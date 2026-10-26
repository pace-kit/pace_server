#![allow(dead_code)]

use std::path::Path;

use pace_server::{authenticated_router, Auth, Store};
use reqwest::{Client, Response};
use sha2::{Digest, Sha256};
use tokio::{net::TcpListener, task::JoinHandle};

pub const PROJECT: &[u8] = include_bytes!("../fixtures/core-scene-v1.json");

pub fn edited(title: &str) -> Vec<u8> {
    let mut value: serde_json::Value = serde_json::from_slice(PROJECT).unwrap();
    value["title"] = title.into();
    serde_json::to_vec_pretty(&value).unwrap()
}

pub const ALICE: &str = "alice-test-token-00000000000000000000000000000000";
pub const BOB: &str = "bob-test-token-0000000000000000000000000000000000";
pub const EVE: &str = "eve-test-token-0000000000000000000000000000000000";

pub fn auth_json() -> Vec<u8> {
    let tokens: Vec<_> = [("alice", ALICE), ("bob", BOB), ("eve", EVE)].into_iter()
        .map(|(principal, token)| serde_json::json!({"principal":principal,"sha256": format!("{:x}", Sha256::digest(token.as_bytes()))})).collect();
    serde_json::to_vec(&serde_json::json!({"tokens":tokens})).unwrap()
}

pub fn client(token: &str) -> Client {
    let mut headers = reqwest::header::HeaderMap::new();
    let mut value = reqwest::header::HeaderValue::from_str(&format!("Bearer {token}")).unwrap();
    value.set_sensitive(true);
    headers.insert(reqwest::header::AUTHORIZATION, value);
    Client::builder()
        .default_headers(headers)
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .unwrap()
}

pub struct Server {
    pub base: String,
    pub client: Client,
    task: JoinHandle<()>,
}

impl Server {
    pub async fn start(path: &Path) -> Self {
        let app = authenticated_router(
            Store::open(path).unwrap(),
            Auth::from_json(&auth_json()).unwrap(),
        )
        .layer(pace_server::cors::cors("https://editor.example").unwrap());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        Self {
            base,
            client: client(ALICE),
            task,
        }
    }

    pub async fn create(&self, bytes: impl Into<Vec<u8>>) -> Response {
        self.client
            .post(format!("{}/v1/projects", self.base))
            .header("Content-Type", "application/json")
            .body(bytes.into())
            .send()
            .await
            .unwrap()
    }

    pub async fn project(&self) -> (String, String) {
        let response = self.create(PROJECT).await;
        assert_eq!(response.status(), 201);
        let location = response.headers()["Location"].to_str().unwrap().to_owned();
        let etag = response.headers()["ETag"].to_str().unwrap().to_owned();
        (format!("{}{location}", self.base), etag)
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}
