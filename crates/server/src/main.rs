//! `llmtz`: the headless LocalLLMTranslator.
//!
//! `llmtz serve` starts the same control plane as the desktop app (database, queue,
//! worker pool, sidecar supervisor) and serves the built UI together with the HTTP
//! command API (`POST /api/<command>`, `GET /api/events`). The desktop shell keeps
//! working; both share `app_lib::build_state`. See `PLAN.md` §12.3.

mod api;
mod events;
mod files;

use std::convert::Infallible;
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use app_lib::AppState;
use axum::extract::{DefaultBodyLimit, Path, Request, State};
use axum::http::{header, StatusCode};
use axum::middleware::{self, Next};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use clap::{Args, Parser, Subcommand};
use events::BroadcastEmitter;
use futures_util::stream::{self, Stream};
use serde_json::{json, Value};
use tokio::sync::broadcast;
use tower_http::services::{ServeDir, ServeFile};

/// The events kept for a client that is briefly busy; a slower one skips ahead.
const EVENT_BUFFER: usize = 1024;

#[derive(Debug, Parser)]
#[command(
    name = "llmtz",
    version,
    about = "LocalLLMTranslator without the desktop shell"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Run the control plane and serve the UI and the command API.
    Serve(ServeArgs),
}

#[derive(Debug, Args)]
struct ServeArgs {
    /// Interface to bind. Anything but loopback requires a token.
    #[arg(long, default_value = "127.0.0.1")]
    host: String,
    #[arg(long, default_value_t = 4321)]
    port: u16,
    /// Database and project files. Defaults to the desktop app's data directory.
    #[arg(long)]
    data_dir: Option<PathBuf>,
    /// Directory of the built frontend (`ui/dist`).
    #[arg(long)]
    ui_dir: Option<PathBuf>,
    /// Shared secret for `/api/*`; required when binding a non-loopback host.
    #[arg(long)]
    token: Option<String>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Serve(args) => serve(args).await,
    }
}

async fn serve(args: ServeArgs) -> anyhow::Result<()> {
    let data_dir = args.data_dir.unwrap_or_else(default_data_dir);
    app_lib::logging::init(&data_dir);
    let ui_dir = resolve_ui_dir(args.ui_dir)?;

    let remote = !is_loopback(&args.host);
    if remote && args.token.is_none() {
        anyhow::bail!(
            "--host {} exposes the app beyond this machine: pass --token <secret> or bind 127.0.0.1",
            args.host
        );
    }
    let token = args.token;

    // The control plane emits through a broadcast emitter, so every connected browser
    // receives the same `job://progress`, `log://line`, … the desktop webview does.
    let (emitter, trait_emitter) = events::shared_emitter(EVENT_BUFFER);
    let state = Arc::new(app_lib::build_state(data_dir.clone(), None, trait_emitter).await?);

    let server = ServerState {
        state,
        emitter,
        token: token.clone(),
    };
    // Kept aside for the shutdown after `serve`; `ServerState` itself is moved into the router.
    let app_state = server.state.clone();
    let app = app_router(server, &ui_dir);

    let address: SocketAddr = format!("{}:{}", args.host, args.port).parse()?;
    let listener = tokio::net::TcpListener::bind(address).await?;
    let local = listener.local_addr()?;
    tracing::info!(%local, ui = %ui_dir.display(), data = %data_dir.display(), "llmtz serve");
    println!("LocalLLMTranslator on http://{local}");
    if let Some(token) = &token {
        println!("API token: {token}");
    }

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    // Release the worker pool and the sidecar child deterministically, like the desktop
    // shell does on exit.
    tracing::info!("shutting down: stopping worker pool and sidecar");
    server_shutdown(&app_state);
    Ok(())
}

#[derive(Clone)]
struct ServerState {
    state: Arc<AppState>,
    emitter: Arc<BroadcastEmitter>,
    token: Option<String>,
}

/// The whole HTTP surface: the command API under `/api`, the events stream, and the built
/// UI as static files with an SPA fallback.
fn app_router(server: ServerState, ui_dir: &std::path::Path) -> Router {
    let api = Router::new()
        .route("/{command}", post(run_command))
        .route("/events", get(stream_events))
        .route(
            "/upload",
            post(files::upload_file).layer(DefaultBodyLimit::max(files::MAX_UPLOAD_BYTES)),
        )
        .route("/download", get(files::download_file))
        .route_layer(middleware::from_fn_with_state(
            server.clone(),
            require_token,
        ))
        .with_state(server);

    let index = ui_dir.join("index.html");
    Router::new()
        .nest("/api", api)
        .fallback_service(ServeDir::new(ui_dir).fallback(ServeFile::new(index)))
}

/// One command call: the body is the argument object `invoke()` would send.
async fn run_command(
    State(server): State<ServerState>,
    Path(command): Path<String>,
    body: String,
) -> Response {
    let args: Value = if body.trim().is_empty() {
        json!({})
    } else {
        match serde_json::from_str(&body) {
            Ok(value) => value,
            Err(error) => {
                return error_response(app_lib::error::AppError::Invalid(format!(
                    "the request body is not valid JSON: {error}"
                )));
            }
        }
    };
    match api::dispatch(&server.state, &command, &args).await {
        Ok(value) => (StatusCode::OK, Json(value)).into_response(),
        Err(error) => error_response(error),
    }
}

pub(crate) fn error_response(error: app_lib::error::AppError) -> Response {
    let status = api::status_for(&error);
    (status, Json(error)).into_response()
}

/// `GET /api/events`: the control-plane events as Server-Sent Events, one named event
/// per `job://progress`, `log://line`, … exactly as `listen()` receives them.
async fn stream_events(
    State(server): State<ServerState>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let receiver = server.emitter.subscribe();
    let stream = stream::unfold(receiver, |mut receiver| async move {
        loop {
            match receiver.recv().await {
                Ok((name, payload)) => {
                    return Some((
                        Ok(Event::default().event(name).data(payload.to_string())),
                        receiver,
                    ));
                }
                Err(broadcast::error::RecvError::Lagged(skipped)) => {
                    // A client that cannot keep up skips events; the next `metrics://tick`
                    // or explicit refetch reconciles its views.
                    tracing::debug!(skipped, "SSE client lagged");
                }
                Err(broadcast::error::RecvError::Closed) => return None,
            }
        }
    });
    Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
}

/// Reject `/api/*` requests without the configured token. Without a token (loopback
/// bind) everything is allowed.
async fn require_token(
    State(server): State<ServerState>,
    request: Request,
    next: Next,
) -> Response {
    let Some(expected) = server.token.as_deref() else {
        return next.run(request).await;
    };
    let presented = bearer_token(&request).or_else(|| query_token(&request));
    if presented.as_deref() == Some(expected) {
        return next.run(request).await;
    }
    (
        StatusCode::UNAUTHORIZED,
        Json(json!({
            "code": "unauthorized",
            "message": "missing or invalid API token",
            "retryable": false,
        })),
    )
        .into_response()
}

fn bearer_token(request: &Request) -> Option<String> {
    let value = request.headers().get(header::AUTHORIZATION)?;
    let text = value.to_str().ok()?;
    text.strip_prefix("Bearer ").map(str::to_string)
}

fn query_token(request: &Request) -> Option<String> {
    let query = request.uri().query()?;
    query.split('&').find_map(|pair| {
        let (key, value) = pair.split_once('=')?;
        (key == "token").then(|| value.to_string())
    })
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
}

/// Stop the worker pool and the sidecar. Idempotent, like the desktop shell's shutdown.
fn server_shutdown(state: &AppState) {
    state.worker.cancel();
    state.supervisor.shutdown();
}

fn is_loopback(host: &str) -> bool {
    if host == "localhost" {
        return true;
    }
    host.parse::<IpAddr>()
        .map(|ip| ip.is_loopback())
        .unwrap_or(false)
}

/// `$LLMTZ_DATA_DIR` when set, else the desktop app's data directory, so `llmtz serve`
/// shows the same books by default.
fn default_data_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("LLMTZ_DATA_DIR") {
        let trimmed = dir.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }
    let base = if cfg!(windows) {
        std::env::var_os("APPDATA").map(PathBuf::from)
    } else if cfg!(target_os = "macos") {
        std::env::var_os("HOME").map(|home| PathBuf::from(home).join("Library/Application Support"))
    } else {
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share"))
            })
    };
    base.unwrap_or_else(|| PathBuf::from("."))
        .join("org.localllmtranslator.app")
}

/// The built frontend: `--ui-dir`, then `$LLMTZ_UI_DIR`, then the repository path
/// (`ui/dist` next to `crates/server`), which is where `make build-ui` leaves it.
fn resolve_ui_dir(explicit: Option<PathBuf>) -> anyhow::Result<PathBuf> {
    let candidates = [
        explicit,
        std::env::var_os("LLMTZ_UI_DIR").map(PathBuf::from),
        Some(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../ui/dist")),
    ];
    for candidate in candidates.into_iter().flatten() {
        if candidate.join("index.html").is_file() {
            return Ok(candidate);
        }
    }
    anyhow::bail!("no built UI found: run `make build-ui` or pass --ui-dir")
}

#[cfg(test)]
mod tests {
    use super::{bearer_token, default_data_dir, is_loopback, query_token, resolve_ui_dir};
    use axum::body::Body;
    use axum::http::Request;

    fn request(uri: &str, authorization: Option<&str>) -> Request<Body> {
        let mut builder = Request::builder().uri(uri);
        if let Some(value) = authorization {
            builder = builder.header("authorization", value);
        }
        builder.body(Body::empty()).expect("request")
    }

    #[test]
    fn loopback_hosts_are_recognized() {
        assert!(is_loopback("127.0.0.1"));
        assert!(is_loopback("127.0.0.42"));
        assert!(is_loopback("localhost"));
        assert!(is_loopback("::1"));
        assert!(!is_loopback("0.0.0.0"));
        assert!(!is_loopback("192.168.1.10"));
        assert!(!is_loopback("example.com"));
    }

    #[test]
    fn the_token_comes_from_the_bearer_header_or_the_query() {
        let header = request("/api/project_list", Some("Bearer s3cret"));
        assert_eq!(bearer_token(&header).as_deref(), Some("s3cret"));
        // Another scheme is not a bearer token.
        let basic = request("/api/project_list", Some("Basic s3cret"));
        assert!(bearer_token(&basic).is_none());
        // The query carries it for clients that cannot set a header.
        let query = request("/api/events?token=s3cret&x=1", None);
        assert_eq!(query_token(&query).as_deref(), Some("s3cret"));
        assert!(query_token(&request("/api/events", None)).is_none());
    }

    #[test]
    fn the_data_dir_env_override_wins() {
        // SAFETY: the test process owns its environment; the previous value is restored.
        let previous = std::env::var_os("LLMTZ_DATA_DIR");
        std::env::set_var("LLMTZ_DATA_DIR", "/tmp/llmtz-env-test");
        assert_eq!(
            default_data_dir(),
            std::path::PathBuf::from("/tmp/llmtz-env-test")
        );
        match previous {
            Some(value) => std::env::set_var("LLMTZ_DATA_DIR", value),
            None => std::env::remove_var("LLMTZ_DATA_DIR"),
        }
    }

    #[test]
    fn an_explicit_ui_dir_with_an_index_wins() {
        let dir = tempfile::tempdir().expect("temp dir");
        std::fs::write(dir.path().join("index.html"), "<html></html>").expect("index");
        let resolved = resolve_ui_dir(Some(dir.path().to_path_buf())).expect("ui dir");
        assert_eq!(resolved, dir.path());
    }
}

#[cfg(test)]
mod http_tests {
    use super::{app_router, events, ServerState};
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use std::sync::Arc;
    use tower::ServiceExt;

    /// A real control plane on a throwaway data dir, with the sidecar pinned to a path
    /// that cannot exist so the test never spawns a process.
    async fn test_router(token: Option<&str>) -> (axum::Router, tempfile::TempDir) {
        std::env::set_var("LLMTRANSLATOR_SIDECAR", "/nonexistent/llmtz-test-sidecar");
        std::env::set_var("LLMTRANSLATOR_PANDOC_DIR", "/nonexistent/llmtz-test-pandoc");
        let dir = tempfile::tempdir().expect("temp data dir");
        // The same directory doubles as the "built UI": the static fallback needs an index.
        std::fs::write(dir.path().join("index.html"), "<html></html>").expect("index");
        let (emitter, trait_emitter) = events::shared_emitter(64);
        let state = Arc::new(
            app_lib::build_state(dir.path().to_path_buf(), None, trait_emitter)
                .await
                .expect("build state"),
        );
        let server = ServerState {
            state,
            emitter,
            token: token.map(str::to_string),
        };
        (app_router(server, dir.path()), dir)
    }

    fn command(name: &str, body: &str) -> Request<Body> {
        Request::builder()
            .method("POST")
            .uri(format!("/api/{name}"))
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .expect("request")
    }

    #[tokio::test]
    async fn a_command_runs_and_a_missing_row_is_a_404() {
        let (app, _dir) = test_router(None).await;

        let response = app
            .clone()
            .oneshot(command("project_list", "{}"))
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::OK);

        let response = app
            .oneshot(command("project_get", r#"{"id":"missing"}"#))
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn an_unknown_command_is_invalid() {
        let (app, _dir) = test_router(None).await;
        let response = app
            .oneshot(command("not_a_command", "{}"))
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn the_token_gates_the_api_not_the_static_files() {
        let (app, _dir) = test_router(Some("s3cret")).await;

        let response = app
            .clone()
            .oneshot(command("project_list", "{}"))
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

        let authorized = Request::builder()
            .method("POST")
            .uri("/api/project_list")
            .header("content-type", "application/json")
            .header("authorization", "Bearer s3cret")
            .body(Body::from("{}"))
            .expect("request");
        let response = app.clone().oneshot(authorized).await.expect("response");
        assert_eq!(response.status(), StatusCode::OK);

        // Static files stay readable: the browser has to load the app before it can
        // attach the token to its API calls.
        let index = Request::builder()
            .uri("/")
            .body(Body::empty())
            .expect("request");
        let response = app.oneshot(index).await.expect("response");
        assert_eq!(response.status(), StatusCode::OK);
    }
}
