#![allow(dead_code)]

use std::path::Path;

use axum::Extension;
use pace_server::{router, Principal, Store};
use reqwest::{Client, Response};
use tokio::{net::TcpListener, task::JoinHandle};

pub const PROJECT: &[u8] =
    b"{ \"scene\": {\"nodes\":[]}, \"extension\": {\"large\":123456789012345678901234567890} }\n";

pub struct Server {
    pub base: String,
    pub client: Client,
    task: JoinHandle<()>,
}

impl Server {
    pub async fn start(path: &Path) -> Self {
        let app = router(Store::open(path).unwrap()).layer(Extension(Principal("alice".into())));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        Self {
            base,
            client: Client::new(),
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
