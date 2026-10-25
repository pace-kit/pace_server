mod common;

use std::{
    io::{BufRead, BufReader},
    path::Path,
    process::{Child, Command, Stdio},
    sync::mpsc,
    time::Duration,
};

use common::{client, ALICE, BOB, EVE, PROJECT};
use tempfile::tempdir;

struct Process {
    child: Child,
    base: String,
}

impl Process {
    fn start(directory: &Path) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_pace_server"))
            .env("PACE_BIND", "127.0.0.1:0")
            .env("PACE_DATABASE", directory.join("projects.sqlite3"))
            .env("PACE_AUTH_FILE", directory.join("auth.json"))
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let mut process = Self {
            child,
            base: String::new(),
        };
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let mut line = String::new();
            BufReader::new(stdout).read_line(&mut line).unwrap();
            let _ = tx.send(line);
        });
        let line = rx
            .recv_timeout(Duration::from_secs(10))
            .expect("Server did not start");
        process.base = line
            .trim()
            .strip_prefix("Listening on ")
            .expect("Missing listening address")
            .to_owned();
        process
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        // Abrupt process termination also exercises SQLite WAL recovery.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[tokio::test]
async fn project_revision_ownership_and_grants_survive_process_restart() {
    let dir = tempdir().unwrap();
    std::fs::write(dir.path().join("auth.json"), common::auth_json()).unwrap();
    let server = Process::start(dir.path());
    let alice = client(ALICE);
    let bob = client(BOB);
    let created = alice
        .post(format!("{}/v1/projects", server.base))
        .header("Content-Type", "application/json")
        .body(PROJECT)
        .send()
        .await
        .unwrap();
    assert_eq!(created.status(), 201);
    let location = created.headers()["Location"].to_str().unwrap().to_owned();
    let url = format!("{}{location}", server.base);
    assert_eq!(
        alice
            .put(format!("{url}/members/bob"))
            .json(&serde_json::json!({"role":"writer"}))
            .send()
            .await
            .unwrap()
            .status(),
        204
    );
    assert_eq!(
        bob.put(&url)
            .header("If-Match", "\"1\"")
            .header("Content-Type", "application/json")
            .body(PROJECT)
            .send()
            .await
            .unwrap()
            .status(),
        204
    );
    drop(server);

    let server = Process::start(dir.path());
    let url = format!("{}{location}", server.base);
    let response = bob.get(&url).send().await.unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()["ETag"], "\"2\"");
    assert_eq!(response.bytes().await.unwrap().as_ref(), PROJECT);
    assert_eq!(client(EVE).get(&url).send().await.unwrap().status(), 404);
    assert_eq!(
        alice
            .put(&url)
            .header("If-Match", "\"1\"")
            .header("Content-Type", "application/json")
            .body(PROJECT)
            .send()
            .await
            .unwrap()
            .status(),
        412
    );
    assert_eq!(
        alice
            .delete(format!("{url}/members/bob"))
            .send()
            .await
            .unwrap()
            .status(),
        204
    );
    drop(server);

    let server = Process::start(dir.path());
    let url = format!("{}{location}", server.base);
    assert_eq!(bob.get(&url).send().await.unwrap().status(), 404);
    assert_eq!(alice.get(url).send().await.unwrap().status(), 200);
}

#[test]
fn server_refuses_to_start_without_valid_authentication_configuration() {
    let dir = tempdir().unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_pace_server"));
    command.current_dir(dir.path()).env_remove("PACE_AUTH_FILE");
    assert!(!command.output().unwrap().status.success());
    std::fs::write(dir.path().join("auth.json"), "{\"tokens\":[]}").unwrap();
    assert!(!command
        .env("PACE_AUTH_FILE", dir.path().join("auth.json"))
        .output()
        .unwrap()
        .status
        .success());
    assert!(!dir.path().join("pace-projects.sqlite3").exists());
}
