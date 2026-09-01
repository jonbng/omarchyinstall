//! Secure loopback UI used when the native WebView cannot be created.

use crate::{cidata::CidataIdentity, download, platform};
use axum::{
    extract::{Path, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{sse::Event, IntoResponse, Response, Sse},
    routing::{get, post},
    Json, Router,
};
use futures_util::stream::{self, Stream};
use serde::Serialize;
use serde_json::{json, Value};
use std::{
    convert::Infallible,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
use tauri::AppHandle;
use tokio::sync::{broadcast, Mutex};

const SESSION_COOKIE: &str = "omarchy_session";
const IDLE_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Clone)]
struct BrowserState {
    app: AppHandle,
    assets: Arc<tauri::AssetResolver<tauri::Wry>>,
    origin: Arc<str>,
    token: Arc<str>,
    events: broadcast::Sender<download::IsoProgress>,
    operation: Arc<Mutex<()>>,
    last_seen: Arc<Mutex<Instant>>,
    active: Arc<AtomicUsize>,
}

struct ActiveGuard(Arc<AtomicUsize>);

impl ActiveGuard {
    fn new(active: Arc<AtomicUsize>) -> Self {
        active.fetch_add(1, Ordering::SeqCst);
        Self(active)
    }
}

impl Drop for ActiveGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

#[derive(Serialize)]
#[serde(untagged)]
enum ApiResponse {
    Ok { ok: bool, value: Value },
    Err { ok: bool, error: String },
}

pub fn launch(app: &AppHandle, webview_error: &str) -> Result<(), String> {
    let std_listener = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .map_err(|e| format!("bind browser fallback: {e}"))?;
    std_listener
        .set_nonblocking(true)
        .map_err(|e| format!("configure browser fallback: {e}"))?;
    let address = std_listener
        .local_addr()
        .map_err(|e| format!("read browser fallback address: {e}"))?;
    let configured_token = if cfg!(debug_assertions) {
        std::env::var("OMARCHY_TEST_BROWSER_TOKEN").ok()
    } else {
        None
    };
    let token = if let Some(token) = configured_token {
        token
    } else {
        let mut token_bytes = [0u8; 32];
        getrandom::fill(&mut token_bytes)
            .map_err(|e| format!("create browser session token: {e}"))?;
        token_bytes
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    };
    let origin = format!("http://127.0.0.1:{}", address.port());
    let (event_tx, _) = broadcast::channel(64);
    let state = BrowserState {
        app: app.clone(),
        assets: Arc::new(app.asset_resolver()),
        origin: origin.clone().into(),
        token: token.clone().into(),
        events: event_tx,
        operation: Arc::new(Mutex::new(())),
        last_seen: Arc::new(Mutex::new(Instant::now())),
        active: Arc::new(AtomicUsize::new(0)),
    };

    let router = Router::new()
        .route("/", get(index))
        .route("/api/session", post(create_session))
        .route("/api/heartbeat", post(heartbeat))
        .route("/api/events", get(events))
        .route("/api/invoke/{command}", post(invoke))
        .route("/{*path}", get(asset))
        .with_state(state.clone());
    tauri::async_runtime::spawn(async move {
        match tokio::net::TcpListener::from_std(std_listener) {
            Ok(listener) => {
                if let Err(error) = axum::serve(listener, router).await {
                    log::error!("browser fallback server stopped: {error}");
                }
            }
            Err(error) => {
                log::error!("browser fallback could not start: {error}");
            }
        }
    });
    spawn_idle_watch(state);

    let url = format!("{origin}/#token={token}");
    log::warn!("WebView unavailable ({webview_error}); opening browser fallback at {origin}");
    tauri_plugin_opener::open_url(url, None::<&str>)
        .map_err(|e| format!("open browser fallback: {e}"))?;
    Ok(())
}

fn spawn_idle_watch(state: BrowserState) {
    tauri::async_runtime::spawn(async move {
        let mut timer = tokio::time::interval(Duration::from_secs(10));
        loop {
            timer.tick().await;
            if state.active.load(Ordering::SeqCst) > 0 {
                continue;
            }
            if state.last_seen.lock().await.elapsed() >= IDLE_TIMEOUT {
                log::info!("browser fallback idle; exiting");
                state.app.exit(0);
                break;
            }
        }
    });
}

async fn touch(state: &BrowserState) {
    *state.last_seen.lock().await = Instant::now();
}

fn request_origin_is_valid(state: &BrowserState, headers: &HeaderMap) -> bool {
    origin_is_valid(state.origin.as_ref(), headers) && host_is_valid(state.origin.as_ref(), headers)
}

fn origin_is_valid(expected: &str, headers: &HeaderMap) -> bool {
    headers
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value == expected)
}

fn host_is_valid(origin: &str, headers: &HeaderMap) -> bool {
    let expected = origin.strip_prefix("http://").unwrap_or(origin);
    headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value == expected)
}

fn bearer_is_valid(state: &BrowserState, headers: &HeaderMap) -> bool {
    bearer_matches(state.token.as_ref(), headers)
}

fn bearer_matches(expected: &str, headers: &HeaderMap) -> bool {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .is_some_and(|value| value == expected)
}

fn session_is_valid(state: &BrowserState, headers: &HeaderMap) -> bool {
    session_matches(state.token.as_ref(), headers)
}

fn session_matches(expected: &str, headers: &HeaderMap) -> bool {
    headers
        .get(header::COOKIE)
        .and_then(|value| value.to_str().ok())
        .map(|cookies| {
            cookies.split(';').any(|cookie| {
                cookie.trim().strip_prefix(&format!("{SESSION_COOKIE}=")) == Some(expected)
            })
        })
        .unwrap_or(false)
}

async fn create_session(State(state): State<BrowserState>, headers: HeaderMap) -> Response {
    if !request_origin_is_valid(&state, &headers) || !bearer_is_valid(&state, &headers) {
        return StatusCode::FORBIDDEN.into_response();
    }
    touch(&state).await;
    let cookie = format!(
        "{SESSION_COOKIE}={}; HttpOnly; SameSite=Strict; Path=/",
        state.token
    );
    let mut response = StatusCode::NO_CONTENT.into_response();
    response.headers_mut().insert(
        header::SET_COOKIE,
        HeaderValue::from_str(&cookie).expect("session cookie is ASCII"),
    );
    response
}

async fn heartbeat(State(state): State<BrowserState>, headers: HeaderMap) -> Response {
    if !request_origin_is_valid(&state, &headers) || !session_is_valid(&state, &headers) {
        return StatusCode::FORBIDDEN.into_response();
    }
    touch(&state).await;
    StatusCode::NO_CONTENT.into_response()
}

async fn events(State(state): State<BrowserState>, headers: HeaderMap) -> Response {
    if !host_is_valid(state.origin.as_ref(), &headers) || !session_is_valid(&state, &headers) {
        return StatusCode::FORBIDDEN.into_response();
    }
    touch(&state).await;
    let receiver = state.events.subscribe();
    let stream = event_stream(receiver);
    Sse::new(stream)
        .keep_alive(axum::response::sse::KeepAlive::new().interval(Duration::from_secs(15)))
        .into_response()
}

fn event_stream(
    receiver: broadcast::Receiver<download::IsoProgress>,
) -> impl Stream<Item = Result<Event, Infallible>> {
    stream::unfold(receiver, |mut receiver| async move {
        loop {
            match receiver.recv().await {
                Ok(progress) => {
                    let event = Event::default()
                        .event("iso://progress")
                        .json_data(progress)
                        .expect("ISO progress serializes");
                    return Some((Ok(event), receiver));
                }
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => return None,
            }
        }
    })
}

async fn invoke(
    Path(command): Path<String>,
    State(state): State<BrowserState>,
    headers: HeaderMap,
    Json(args): Json<Value>,
) -> Response {
    if !request_origin_is_valid(&state, &headers) || !session_is_valid(&state, &headers) {
        return StatusCode::FORBIDDEN.into_response();
    }
    touch(&state).await;
    let work_state = state.clone();
    let result = tauri::async_runtime::spawn(async move {
        let _active = ActiveGuard::new(work_state.active.clone());
        run_command(&work_state, &command, args).await
    })
    .await
    .map_err(|error| format!("command task failed: {error}"))
    .and_then(|result| result);
    let body = match result {
        Ok(value) => ApiResponse::Ok { ok: true, value },
        Err(error) => ApiResponse::Err { ok: false, error },
    };
    Json(body).into_response()
}

async fn run_command(state: &BrowserState, command: &str, args: Value) -> Result<Value, String> {
    macro_rules! blocking {
        ($expr:expr) => {{
            tokio::task::spawn_blocking(move || $expr)
                .await
                .map_err(|e| e.to_string())?
                .map_err(|e| e.to_string())
                .and_then(|value| serde_json::to_value(value).map_err(|e| e.to_string()))
        }};
    }

    match command {
        "host_info" => blocking!(platform::host_info()),
        "probe_machine" => blocking!(platform::probe_machine()),
        "load_install_state" => blocking!(platform::load_install_state()),
        "relaunch_elevated" => blocking!(platform::relaunch_elevated()),
        "reboot_to_firmware" => blocking!(platform::reboot_to_firmware()),
        "download_iso" => {
            let _operation = state.operation.lock().await;
            let tx = state.events.clone();
            if download::stub_skips_iso() {
                download::skip_iso_download(move |progress| {
                    let _ = tx.send(progress);
                })
                .await
            } else {
                download::download_iso_files(move |progress| {
                    let _ = tx.send(progress);
                })
                .await
                .map(|_| ())
            }
            .map_err(|e| e.to_string())?;
            Ok(Value::Null)
        }
        "pick_local_iso" => blocking!(platform::pick_local_iso()),
        "prepare_local_iso" => {
            let _operation = state.operation.lock().await;
            let path = args
                .get("path")
                .and_then(Value::as_str)
                .ok_or_else(|| "path is required".to_string())?;
            download::prepare_local_iso(std::path::Path::new(path))
                .await
                .map_err(|e| e.to_string())
                .and_then(|value| serde_json::to_value(value).map_err(|e| e.to_string()))
        }
        "verify_iso" => {
            let _operation = state.operation.lock().await;
            let tx = state.events.clone();
            blocking!(if download::stub_skips_iso() {
                download::skip_iso_verify(move |progress| {
                    let _ = tx.send(progress);
                })
            } else {
                download::verify_iso_files(move |progress| {
                    let _ = tx.send(progress);
                })
            })
        }
        "prepare_installer_partition" => {
            let _operation = state.operation.lock().await;
            let allow_bitlocker = args
                .get("allowBitlocker")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            blocking!(platform::prepare_installer_partition(allow_bitlocker))
        }
        "stage_bootloader" => {
            let _operation = state.operation.lock().await;
            blocking!(platform::stage_bootloader())
        }
        "write_cidata" => {
            let _operation = state.operation.lock().await;
            let identity: CidataIdentity =
                serde_json::from_value(args.get("identity").cloned().unwrap_or(Value::Null))
                    .map_err(|e| format!("identity: {e}"))?;
            blocking!(platform::write_cidata(identity))
        }
        "set_boot_next" => {
            let _operation = state.operation.lock().await;
            blocking!(platform::set_boot_next())
        }
        "reboot_to_installer" => {
            let _operation = state.operation.lock().await;
            blocking!(platform::reboot_to_installer())
        }
        "abort_and_rollback" => {
            let _operation = state.operation.lock().await;
            blocking!(platform::abort_and_rollback())
        }
        "export_support_bundle" => blocking!(platform::export_support_bundle()),
        "_version" => Ok(json!(env!("CARGO_PKG_VERSION"))),
        "_shutdown" => {
            let app = state.app.clone();
            tauri::async_runtime::spawn(async move {
                tokio::time::sleep(Duration::from_millis(100)).await;
                app.exit(0);
            });
            Ok(Value::Null)
        }
        _ => Err(format!("unknown command: {command}")),
    }
}

async fn index(State(state): State<BrowserState>) -> Response {
    serve_asset(&state, "index.html")
}

async fn asset(Path(path): Path<String>, State(state): State<BrowserState>) -> Response {
    if path.starts_with("api/") || path.contains("..") {
        return StatusCode::NOT_FOUND.into_response();
    }
    serve_asset(&state, &path)
}

fn serve_asset(state: &BrowserState, path: &str) -> Response {
    let Some(asset) = state.assets.get(path.to_string()) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let mut response = asset.bytes.into_response();
    let headers = response.headers_mut();
    if let Ok(value) = HeaderValue::from_str(&asset.mime_type) {
        headers.insert(header::CONTENT_TYPE, value);
    }
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(
            "default-src 'self'; connect-src 'self'; font-src 'self'; img-src 'self' data: blob:; style-src 'self' 'unsafe-inline'; script-src 'self'; frame-ancestors 'none'",
        ),
    );
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browser_auth_requires_exact_values() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("http://127.0.0.1:49152"),
        );
        headers.insert(header::HOST, HeaderValue::from_static("127.0.0.1:49152"));
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("Bearer secret"),
        );
        headers.insert(
            header::COOKIE,
            HeaderValue::from_static("other=x; omarchy_session=secret"),
        );

        assert!(origin_is_valid("http://127.0.0.1:49152", &headers));
        assert!(!origin_is_valid("http://127.0.0.1:49153", &headers));
        assert!(host_is_valid("http://127.0.0.1:49152", &headers));
        assert!(!host_is_valid("http://127.0.0.1:49153", &headers));
        assert!(bearer_matches("secret", &headers));
        assert!(!bearer_matches("secret2", &headers));
        assert!(session_matches("secret", &headers));
        assert!(!session_matches("sec", &headers));
    }

    #[test]
    fn browser_auth_rejects_missing_headers() {
        let headers = HeaderMap::new();
        assert!(!origin_is_valid("http://127.0.0.1:1", &headers));
        assert!(!host_is_valid("http://127.0.0.1:1", &headers));
        assert!(!bearer_matches("secret", &headers));
        assert!(!session_matches("secret", &headers));
    }
}
