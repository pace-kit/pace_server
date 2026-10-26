use std::{
    path::Path,
    sync::{Arc, Mutex},
    time::Duration,
};

use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use uuid::Uuid;

use crate::{
    error::ApiError,
    project::{validate_project, Role},
};

/// Supplied by a trusted authentication layer, never by a project body or URL.
#[derive(Clone, Debug)]
pub struct Principal(pub String);

#[derive(Clone)]
pub struct Store(Arc<Mutex<Connection>>);

pub struct Snapshot {
    pub id: String,
    pub revision: i64,
    pub body: Vec<u8>,
}

impl Store {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, ApiError> {
        let connection = Connection::open(path)?;
        connection.busy_timeout(Duration::from_secs(5))?;
        connection.execute_batch(
            "PRAGMA foreign_keys = ON;
             PRAGMA journal_mode = WAL;
             PRAGMA synchronous = FULL;
             CREATE TABLE IF NOT EXISTS projects (
                 id TEXT PRIMARY KEY,
                 owner TEXT NOT NULL,
                 revision INTEGER NOT NULL CHECK (revision > 0),
                 body BLOB NOT NULL
             );
             CREATE TABLE IF NOT EXISTS members (
                 project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
                 principal TEXT NOT NULL,
                 role TEXT NOT NULL CHECK (role IN ('reader', 'writer')),
                 PRIMARY KEY (project_id, principal)
             );",
        )?;
        Ok(Self(Arc::new(Mutex::new(connection))))
    }

    /// SQLite work runs off the async executor. IMMEDIATE transactions also serialize
    /// compare-and-swap across independent server processes using the same database.
    async fn run<T: Send + 'static>(
        &self,
        operation: impl FnOnce(&mut Connection) -> Result<T, ApiError> + Send + 'static,
    ) -> Result<T, ApiError> {
        let connection = self.0.clone();
        tokio::task::spawn_blocking(move || {
            let mut connection = connection.lock().map_err(|_| ApiError::internal())?;
            operation(&mut connection)
        })
        .await
        .map_err(|_| ApiError::internal())?
    }

    pub async fn create(&self, principal: Principal, body: Vec<u8>) -> Result<Snapshot, ApiError> {
        self.run(move |connection| {
            validate_project(&body)?;
            let id = Uuid::new_v4().to_string();
            connection.execute(
                "INSERT INTO projects (id, owner, revision, body) VALUES (?1, ?2, 1, ?3)",
                params![id, principal.0, body],
            )?;
            Ok(Snapshot {
                id,
                revision: 1,
                body,
            })
        })
        .await
    }

    pub async fn get(&self, principal: Principal, id: String) -> Result<Snapshot, ApiError> {
        self.run(move |connection| {
            connection.query_row(
                "SELECT revision, body FROM projects WHERE id = ?1 AND
                 (owner = ?2 OR EXISTS (SELECT 1 FROM members WHERE project_id = ?1 AND principal = ?2))",
                params![id, principal.0],
                |row| Ok(Snapshot { id: id.clone(), revision: row.get(0)?, body: row.get(1)? }),
            ).optional()?.ok_or_else(ApiError::not_found)
        }).await
    }

    pub async fn update(
        &self,
        principal: Principal,
        id: String,
        expected: i64,
        body: Vec<u8>,
    ) -> Result<i64, ApiError> {
        self.run(move |connection| {
            validate_project(&body)?;
            let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let (owner, revision): (String, i64) = tx
                .query_row(
                    "SELECT owner, revision FROM projects WHERE id = ?1",
                    [&id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?
                .ok_or_else(ApiError::not_found)?;
            if owner != principal.0 {
                let role: Option<String> = tx
                    .query_row(
                        "SELECT role FROM members WHERE project_id = ?1 AND principal = ?2",
                        params![id, principal.0],
                        |row| row.get(0),
                    )
                    .optional()?;
                match role.as_deref() {
                    Some("writer") => (),
                    Some(_) => return Err(ApiError::forbidden()),
                    None => return Err(ApiError::not_found()),
                }
            }
            if expected != revision {
                return Err(ApiError::conflict(revision));
            }
            let next = revision.checked_add(1).ok_or_else(ApiError::internal)?;
            tx.execute(
                "UPDATE projects SET body = ?1, revision = ?2 WHERE id = ?3",
                params![body, next, id],
            )?;
            tx.commit()?;
            Ok(next)
        })
        .await
    }

    /// Only the owner may grant/revoke access. ACLs and revisions are checked under
    /// the same database write lock, so a revocation cannot race past a later write.
    pub async fn set_member(
        &self,
        principal: Principal,
        id: String,
        member: String,
        role: Option<Role>,
    ) -> Result<(), ApiError> {
        self.run(move |connection| {
            let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let owner: String = tx
                .query_row("SELECT owner FROM projects WHERE id = ?1", [&id], |row| {
                    row.get(0)
                })
                .optional()?
                .ok_or_else(ApiError::not_found)?;
            if owner != principal.0 {
                return Err(ApiError::not_found());
            }
            if member == owner {
                return Err(ApiError::bad_request(
                    "invalid_member",
                    "The owner's access cannot be changed",
                ));
            }
            match role {
                Some(role) => {
                    tx.execute(
                        "INSERT INTO members (project_id, principal, role) VALUES (?1, ?2, ?3)
                         ON CONFLICT (project_id, principal) DO UPDATE SET role = excluded.role",
                        params![id, member, role.as_str()],
                    )?;
                }
                None => {
                    tx.execute(
                        "DELETE FROM members WHERE project_id = ?1 AND principal = ?2",
                        params![id, member],
                    )?;
                }
            }
            tx.commit()?;
            Ok(())
        })
        .await
    }
}
