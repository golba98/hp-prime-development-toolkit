//! Loopback-only viewer. The server has no file-loading or execution endpoint.
use axum::{
    Router,
    extract::{
        Query, State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    http::{HeaderMap, HeaderValue, StatusCode},
    response::{Html, IntoResponse, Response},
    routing::get,
};
use futures_util::SinkExt;
use hpdev_core::{Control, Event};
use serde::Deserialize;
use serde_json::json;
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        mpsc,
    },
    thread::JoinHandle,
};
use tokio::sync::{broadcast, oneshot};

#[derive(Default)]
struct Replay {
    frame: Option<Arc<String>>,
    dialog: Option<Arc<String>>,
    status: Option<Arc<String>>,
}
struct Shared {
    token: String,
    origin: String,
    events: broadcast::Sender<Arc<String>>,
    replay: Mutex<Replay>,
    controls: mpsc::SyncSender<Control>,
    epoch: AtomicU64,
    clients: AtomicUsize,
    closed: AtomicBool,
}
pub struct BrowserViewer {
    shared: Arc<Shared>,
    commands: mpsc::Receiver<Control>,
    shutdown: Option<oneshot::Sender<()>>,
    thread: Option<JoinHandle<()>>,
    ever_connected: AtomicBool,
    cancel_sent: AtomicBool,
    disconnected_since: Mutex<Option<std::time::Instant>>,
}

impl BrowserViewer {
    pub fn new(port: u16, no_open: bool) -> Result<Self, String> {
        let mut bytes = [0u8; 32];
        getrandom::fill(&mut bytes).map_err(|e| e.to_string())?;
        let token = bytes.iter().map(|b| format!("{b:02x}")).collect::<String>();
        let listener = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port))
            .map_err(|e| e.to_string())?;
        let port = listener.local_addr().map_err(|e| e.to_string())?.port();
        listener.set_nonblocking(true).map_err(|e| e.to_string())?;
        let (events, _) = broadcast::channel(8);
        let (tx, rx) = mpsc::sync_channel(64);
        let shared = Arc::new(Shared {
            token,
            origin: format!("http://127.0.0.1:{port}"),
            events,
            replay: Mutex::new(Replay::default()),
            controls: tx,
            epoch: AtomicU64::new(0),
            clients: AtomicUsize::new(0),
            closed: AtomicBool::new(false),
        });
        let (stop, stopped) = oneshot::channel();
        let state = shared.clone();
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let thread = std::thread::spawn(move || {
            let runtime = match tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
            {
                Ok(r) => r,
                Err(e) => {
                    let _ = ready_tx.send(Err(e.to_string()));
                    return;
                }
            };
            runtime.block_on(async move {
                let listener = match tokio::net::TcpListener::from_std(listener) {
                    Ok(l) => l,
                    Err(e) => {
                        let _ = ready_tx.send(Err(e.to_string()));
                        return;
                    }
                };
                let app = Router::new()
                    .route("/", get(index))
                    .route("/socket", get(upgrade))
                    .with_state(state);
                let _ = ready_tx.send(Ok(()));
                let _ = axum::serve(listener, app)
                    .with_graceful_shutdown(async {
                        let _ = stopped.await;
                    })
                    .await;
            });
        });
        ready_rx.recv().map_err(|e| e.to_string())??;
        let viewer = Self {
            shared,
            commands: rx,
            shutdown: Some(stop),
            thread: Some(thread),
            ever_connected: AtomicBool::new(false),
            cancel_sent: AtomicBool::new(false),
            disconnected_since: Mutex::new(None),
        };
        eprintln!("Browser preview: {}", viewer.url());
        if !no_open && let Err(e) = open::that(viewer.url()) {
            eprintln!("Cannot open browser automatically: {e}; open the URL above");
        }
        Ok(viewer)
    }
    pub fn url(&self) -> String {
        format!("{}/?token={}", self.shared.origin, self.shared.token)
    }
    pub fn apply(&self, event: &Event) {
        let message = Arc::new(
            json!({"epoch": self.shared.epoch.load(Ordering::Acquire), "event": event}).to_string(),
        );
        let mut replay = self.shared.replay.lock().unwrap();
        match event {
            Event::Frame { .. } => replay.frame = Some(message.clone()),
            Event::Dialog { .. } => replay.dialog = Some(message.clone()),
            Event::Final { .. } => {
                replay.status = Some(message.clone());
                replay.dialog = None;
            }
            _ => {}
        }
        drop(replay);
        let _ = self.shared.events.send(message);
    }
    pub fn poll(&self) -> Vec<Control> {
        let mut commands: Vec<_> = self.commands.try_iter().collect();
        let mut disconnected = self.disconnected_since.lock().unwrap();
        if self.shared.clients.load(Ordering::Acquire) > 0 {
            self.ever_connected.store(true, Ordering::Release);
            self.cancel_sent.store(false, Ordering::Release);
            *disconnected = None;
        } else if self.ever_connected.load(Ordering::Acquire) {
            let since = disconnected.get_or_insert_with(std::time::Instant::now);
            if since.elapsed() >= std::time::Duration::from_secs(2)
                && !self.cancel_sent.swap(true, Ordering::AcqRel)
            {
                commands.push(Control::Cancel);
            }
        }
        commands
    }
    pub fn ready(&self) -> bool {
        if self.shared.clients.load(Ordering::Acquire) > 0 {
            self.ever_connected.store(true, Ordering::Release);
        }
        self.ever_connected.load(Ordering::Acquire)
    }
    pub fn reset(&self) {
        let epoch = self.shared.epoch.fetch_add(1, Ordering::AcqRel) + 1;
        *self.shared.replay.lock().unwrap() = Replay::default();
        for _ in self.commands.try_iter() {}
        let _ = self.shared.events.send(Arc::new(
            json!({"epoch": epoch, "event": {"kind": "reset"}}).to_string(),
        ));
    }
}
impl Drop for BrowserViewer {
    fn drop(&mut self) {
        self.shared.closed.store(true, Ordering::Release);
        if let Some(stop) = self.shutdown.take() {
            let _ = stop.send(());
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[derive(Deserialize)]
struct Auth {
    token: Option<String>,
}
fn authorized(shared: &Shared, auth: &Auth, headers: &HeaderMap, websocket: bool) -> bool {
    let host = shared.origin.trim_start_matches("http://");
    auth.token.as_deref() == Some(&shared.token)
        && headers.get("host").and_then(|v| v.to_str().ok()) == Some(host)
        && match headers.get("origin").and_then(|v| v.to_str().ok()) {
            Some(origin) => origin == shared.origin,
            None => !websocket,
        }
}
async fn index(
    State(shared): State<Arc<Shared>>,
    Query(auth): Query<Auth>,
    headers: HeaderMap,
) -> Response {
    if !authorized(&shared, &auth, &headers, false) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let mut response =
        Html(include_str!("../assets/viewer.html").replace("__NONCE__", &shared.token))
            .into_response();
    let content_security_policy = format!(
        concat!(
            "default-src 'none'; script-src 'nonce-{}'; style-src 'unsafe-inline'; ",
            "connect-src 'self'; base-uri 'none'; frame-ancestors 'none'; form-action 'none'",
        ),
        shared.token,
    );
    response.headers_mut().insert(
        "content-security-policy",
        HeaderValue::from_str(&content_security_policy).expect("hex nonce"),
    );
    response
        .headers_mut()
        .insert("cache-control", HeaderValue::from_static("no-store"));
    response
        .headers_mut()
        .insert("referrer-policy", HeaderValue::from_static("no-referrer"));
    response
}
async fn upgrade(
    State(shared): State<Arc<Shared>>,
    Query(auth): Query<Auth>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    if !authorized(&shared, &auth, &headers, true) || shared.clients.load(Ordering::Acquire) >= 4 {
        return StatusCode::FORBIDDEN.into_response();
    }
    ws.max_message_size(64 * 1024)
        .on_upgrade(move |socket| session(socket, shared))
        .into_response()
}
async fn deliver(socket: &mut WebSocket, text: String) -> bool {
    matches!(
        tokio::time::timeout(
            std::time::Duration::from_millis(500),
            socket.send(Message::Text(text.into()))
        )
        .await,
        Ok(Ok(()))
    )
}

async fn session(mut socket: WebSocket, shared: Arc<Shared>) {
    shared.clients.fetch_add(1, Ordering::AcqRel);
    let mut events = shared.events.subscribe();
    let replay = {
        let r = shared.replay.lock().unwrap();
        [r.frame.clone(), r.status.clone(), r.dialog.clone()]
    };
    for message in replay.into_iter().flatten() {
        if !deliver(&mut socket, message.as_str().to_owned()).await {
            shared.clients.fetch_sub(1, Ordering::AcqRel);
            return;
        }
    }
    let mut tick = tokio::time::interval(std::time::Duration::from_millis(100));
    loop {
        tokio::select! {
            // Flush a queued final observation before orderly shutdown.
            biased;
            event = events.recv() => {
                match event {
                    Ok(message) => {
                        if !deliver(&mut socket, message.as_str().to_owned()).await {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        let message = json!({
                            "event": {
                                "kind": "viewer_error",
                                "message": "viewer fell behind; reconnect to resynchronize",
                            },
                        });
                        let _ = deliver(&mut socket, message.to_string()).await;
                        break;
                    }
                    Err(_) => break,
                }
            }
            incoming = socket.recv() => {
                match incoming {
                    Some(Ok(Message::Text(text))) => {
                        let Ok(value) = serde_json::from_str::<serde_json::Value>(text.as_str()) else {
                            continue;
                        };
                        if value["epoch"].as_u64() != Some(shared.epoch.load(Ordering::Acquire)) {
                            continue;
                        }
                        let Ok(command) = serde_json::from_value::<Control>(value["control"].clone()) else {
                            continue;
                        };
                        // A browser never controls the debugger or loads a file.
                        if matches!(command, Control::Debug { .. }) {
                            continue;
                        }
                        if matches!(command, Control::Dialog { .. }) {
                            shared.replay.lock().unwrap().dialog = None;
                        }
                        if shared.controls.try_send(command).is_err() {
                            let message = json!({
                                "event": {"kind": "viewer_error", "message": "input queue full"},
                            });
                            if !deliver(&mut socket, message.to_string()).await {
                                break;
                            }
                        }
                    }
                    Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                    _ => {}
                }
            }
            _ = tick.tick() => {
                if shared.closed.load(Ordering::Acquire) {
                    break;
                }
            }
        }
    }
    let _ = tokio::time::timeout(std::time::Duration::from_millis(250), socket.close()).await;
    shared.clients.fetch_sub(1, Ordering::AcqRel);
}
