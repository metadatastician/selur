// SPDX-License-Identifier: MPL-2.0
use assert_cmd::Command;
use axum::{
    extract::{Extension, Json, Path},
    http::StatusCode,
    routing::{get, post},
    Router,
};
use serde_json::{json, Value};
use std::{
    net::SocketAddr,
    os::unix::fs::PermissionsExt,
    sync::Arc,
};
use tokio::{
    net::TcpListener,
    sync::{Mutex, oneshot},
};
use tower::ServiceBuilder;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn parity_stack_acceptance() -> anyhow::Result<()> {
    let containers = Arc::new(Mutex::new(Vec::new()));
    let mcp_calls = Arc::new(Mutex::new(Vec::new()));

    let (svalinn_addr, svalinn_shutdown) =
        spawn_svalinn_server(containers.clone()).await?;
    let (vordr_addr, vordr_shutdown) = spawn_vordr_http_server(containers.clone()).await?;
    let (mcp_addr, mcp_shutdown) = spawn_vordr_mcp_server(mcp_calls.clone()).await?;

    // Test scaffolding: `up` shells out to the `ct` (Cerro Torre) binary for
    // `verify` and opens each service's `.ctp` bundle for `bundle_digest`. Neither
    // exists in CI, so provide a stub `ct` on PATH plus fixture bundle files in a
    // hermetic tempdir that doubles as the child's working directory (the product
    // resolves bundle paths, hence the fixtures, relative to CWD).
    let workdir = tempfile::tempdir()?;
    let work = workdir.path().to_path_buf();

    let ct_stub = work.join("ct");
    std::fs::write(&ct_stub, "#!/bin/sh\nexit 0\n")?;
    std::fs::set_permissions(&ct_stub, std::fs::Permissions::from_mode(0o755))?;

    // Fixture bundles, named to match each service `image` in compose.toml.
    std::fs::write(work.join("nginx:latest.ctp"), b"parity-nginx-bundle")?;
    std::fs::write(work.join("redis:latest.ctp"), b"parity-redis-bundle")?;

    // Address the compose file absolutely, since the child runs from the tempdir.
    let compose_file = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("examples/parity/compose.toml");

    // A closure cannot express the higher-ranked signature
    // `for<'a> Fn(&'a mut Command) -> &'a mut Command`, so use a function item,
    // which ties the returned borrow to the input via lifetime elision.
    fn run_env<'a>(
        cmd: &'a mut Command,
        svalinn_addr: SocketAddr,
        mcp_addr: SocketAddr,
        workdir: &std::path::Path,
    ) -> &'a mut Command {
        cmd.env("SVALINN_URL", format!("http://{}", svalinn_addr));
        cmd.env("VORDR_MCP_URL", format!("http://{}", mcp_addr));
        // Put the stub `ct` (living in `workdir`) first on PATH, and run the child
        // from `workdir` so the `.ctp` bundle fixtures resolve as the relative
        // paths that `up` opens.
        cmd.env(
            "PATH",
            format!(
                "{}:{}",
                workdir.display(),
                std::env::var("PATH").unwrap_or_default()
            ),
        );
        cmd.current_dir(workdir);
        cmd
    }

    run_env(
        &mut Command::cargo_bin("selur-compose")?
            .arg("-f")
            .arg(&compose_file)
            .arg("up")
            .arg("-d"),
        svalinn_addr,
        mcp_addr,
        &work,
    )
    .assert()
    .success();

    run_env(
        &mut Command::cargo_bin("selur-compose")?
            .arg("-f")
            .arg(&compose_file)
            .arg("down")
            .arg("-v")
            .env("VORDR_URL", format!("http://{}", vordr_addr)),
        svalinn_addr,
        mcp_addr,
        &work,
    )
    .assert()
    .success();

    let calls = {
        let lock = mcp_calls.lock().await;
        lock.clone()
    };

    assert!(calls.contains(&"vordr_network_create".to_string()));
    assert!(calls.contains(&"vordr_volume_create".to_string()));
    assert!(calls.contains(&"vordr_network_rm".to_string()));
    assert!(calls.contains(&"vordr_volume_rm".to_string()));

    let _ = svalinn_shutdown.send(());
    let _ = vordr_shutdown.send(());
    let _ = mcp_shutdown.send(());

    Ok(())
}

async fn spawn_svalinn_server(
    containers: Arc<Mutex<Vec<String>>>,
) -> anyhow::Result<(SocketAddr, oneshot::Sender<()>)> {
    let app = Router::new()
        .route("/api/v2/run", post({
            let state = containers.clone();
            move |Json(payload): Json<Value>| {
                let state = state.clone();
                async move {
                    let service_name = payload
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("service");
                    let container_id = format!("parity_{}", service_name);
                    {
                        let mut lock = state.lock().await;
                        lock.push(container_id.clone());
                    }
                    (
                        StatusCode::OK,
                        Json(json!({
                            "container_id": container_id,
                            "status": "running"
                        })),
                    )
                }
            }
        }))
        .route("/api/v1/containers/:id", get(|Path(_): Path<String>| async {
            (
                StatusCode::OK,
                Json(json!({
                    "status": "running"
                })),
            )
        }))
        .route("/api/v1/containers/:id/stop", post(|Path(_): Path<String>| async {
            StatusCode::OK
        }))
        .layer(ServiceBuilder::new().layer(Extension(containers.clone())));

    spawn_router(app).await
}

async fn spawn_vordr_http_server(
    containers: Arc<Mutex<Vec<String>>>,
) -> anyhow::Result<(SocketAddr, oneshot::Sender<()>)> {
    let app = Router::new()
        .route(
            "/api/v1/containers",
            get({
                let containers = containers.clone();
                move || {
                    let containers = containers.clone();
                    async move {
                        let list: Vec<Value> = {
                            let guard = containers.lock().await;
                            guard
                                .iter()
                                .map(|id| {
                                    json!({
                                        "container_id": id,
                                        "state": "running"
                                    })
                                })
                                .collect()
                        };
                        (StatusCode::OK, Json(Value::Array(list)))
                    }
                }
            }),
        )
        .layer(ServiceBuilder::new().layer(Extension(containers.clone())));

    spawn_router(app).await
}

async fn spawn_vordr_mcp_server(
    calls: Arc<Mutex<Vec<String>>>,
) -> anyhow::Result<(SocketAddr, oneshot::Sender<()>)> {
    let app = Router::new()
        .route("/", post({
            let calls = calls.clone();
            move |Json(payload): Json<Value>| {
                let calls = calls.clone();
                async move {
                    if let Some(name) = payload
                        .get("params")
                        .and_then(|params| params.get("name"))
                        .and_then(|v| v.as_str())
                    {
                        let mut lock = calls.lock().await;
                        lock.push(name.to_string());
                    }
                    let response = json!({
                        "jsonrpc": "2.0",
                        "id": payload.get("id").cloned().unwrap_or(Value::Null),
                        "result": {}
                    });
                    (StatusCode::OK, Json(response))
                }
            }
        }))
        .layer(ServiceBuilder::new().layer(Extension(calls.clone())));

    spawn_router(app).await
}

async fn spawn_router(
    app: Router,
) -> anyhow::Result<(SocketAddr, oneshot::Sender<()>)> {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
    let addr = listener.local_addr()?;
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    // `axum::Server::from_tcp` (re-exported from hyper) takes a `std::net::TcpListener`;
    // `tokio::net::TcpListener::into_std` yields one that is already in non-blocking mode.
    let server = axum::Server::from_tcp(listener.into_std()?)?
        .serve(app.into_make_service())
        .with_graceful_shutdown(async {
            let _ = shutdown_rx.await;
        });
    tokio::spawn(server);
    Ok((addr, shutdown_tx))
}
