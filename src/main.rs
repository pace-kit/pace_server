use std::{env, error::Error, io};

use pace_server::{authenticated_router, Auth, Store};
use tokio::net::TcpListener;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let config_path = env::var("PACE_AUTH_FILE").map_err(|_| "PACE_AUTH_FILE is required")?;
    let auth_bytes = std::fs::read(config_path)?;
    let auth = Auth::from_json(&auth_bytes)?;
    let database = env::var("PACE_DATABASE").unwrap_or_else(|_| "pace-projects.sqlite3".into());
    if database.is_empty() || database == ":memory:" {
        return Err("PACE_DATABASE must name a persistent SQLite file".into());
    }
    let store =
        Store::open(database).map_err(|_| io::Error::other("Cannot open project database"))?;
    let bind = env::var("PACE_BIND").unwrap_or_else(|_| "127.0.0.1:3000".into());
    let listener = TcpListener::bind(bind).await?;
    println!("Listening on http://{}", listener.local_addr()?);
    axum::serve(listener, authenticated_router(store, auth))
        .with_graceful_shutdown(shutdown())
        .await?;
    Ok(())
}

async fn shutdown() {
    let interrupt = async {
        tokio::signal::ctrl_c()
            .await
            .expect("Cannot install Ctrl-C handler")
    };
    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("Cannot install SIGTERM handler")
            .recv()
            .await;
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! { _ = interrupt => {}, _ = terminate => {} }
}
