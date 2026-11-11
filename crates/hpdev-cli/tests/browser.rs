#![cfg(feature = "web")]
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    io::{BufRead, BufReader, Read, Write},
    net::TcpStream,
    process::{Child, Command, Stdio},
    sync::mpsc,
    time::Duration,
};
use tungstenite::{Message, client::IntoClientRequest, http::HeaderValue, stream::MaybeTlsStream};
struct Server {
    child: Child,
    url: String,
    stderr: Option<std::thread::JoinHandle<String>>,
}
impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
impl Server {
    fn start(file: &str) -> Self {
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let mut child = Command::new(env!("CARGO_BIN_EXE_hpdev"))
            .current_dir(root)
            .args([
                "preview",
                file,
                "--web",
                "--no-open",
                "--exit-on-finish",
                "--json",
                "--max-time",
                "5",
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let stderr = child.stderr.take().unwrap();
        let (tx, rx) = mpsc::sync_channel(1);
        let errors = std::thread::spawn(move || {
            let mut text = String::new();
            for line in BufReader::new(stderr).lines() {
                let line = line.unwrap();
                if let Some(url) = line.strip_prefix("Browser preview: ") {
                    let _ = tx.send(url.to_owned());
                }
                text.push_str(&line);
                text.push('\n');
            }
            text
        });
        let url = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        Self {
            child,
            url,
            stderr: Some(errors),
        }
    }
    fn socket(&self) -> tungstenite::WebSocket<MaybeTlsStream<TcpStream>> {
        let origin = self.url.split("/?").next().unwrap();
        let url = self.url.replace("http:", "ws:").replace("/?", "/socket?");
        let mut request = url.into_client_request().unwrap();
        request
            .headers_mut()
            .insert("Origin", HeaderValue::from_str(origin).unwrap());
        let (mut socket, _) = tungstenite::connect(request).unwrap();
        if let MaybeTlsStream::Plain(stream) = socket.get_mut() {
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
        }
        socket
    }
    fn result(mut self) -> Value {
        let mut text = String::new();
        self.child
            .stdout
            .take()
            .unwrap()
            .read_to_string(&mut text)
            .unwrap();
        let status = self.child.wait().unwrap();
        let report: Value = serde_json::from_str(&text).unwrap_or_else(|e| panic!("{e}: {text}"));
        assert_eq!(
            status.code().unwrap() as u64,
            report["exit_code"].as_u64().unwrap()
        );
        let _ = self.stderr.take().unwrap().join().unwrap();
        report
    }
}
fn next(socket: &mut tungstenite::WebSocket<MaybeTlsStream<TcpStream>>) -> Value {
    loop {
        match socket.read().unwrap() {
            Message::Text(text) => return serde_json::from_str(text.as_str()).unwrap(),
            Message::Ping(bytes) => {
                socket.send(Message::Pong(bytes)).unwrap();
            }
            other => panic!("unexpected websocket message {other:?}"),
        }
    }
}

#[test]
fn live_browser_transport_delivers_real_frames_before_completion() {
    let server = Server::start("examples/LIVE.hpprgm");
    let mut socket = server.socket();
    let mut colors = BTreeSet::new();
    let mut sequence = 0;
    loop {
        let packet = next(&mut socket);
        let event = &packet["event"];
        if event["kind"] == "frame" {
            let n = event["sequence"].as_u64().unwrap();
            assert_eq!(n, sequence + 1);
            sequence = n;
            let pixels = event["pixels"].as_array().unwrap();
            assert_eq!(pixels.len(), 320 * 240);
            colors.insert(pixels[0].as_u64().unwrap());
        }
        if event["kind"] == "final" {
            break;
        }
    }
    for red in [40u64, 80, 120, 160, 200] {
        assert!(colors.contains(&(red << 16)));
    }
    let report = server.result();
    assert_eq!(report["status"], "success");
    assert_eq!(report["frames_presented"], sequence);
}

#[test]
fn browser_dialogs_accept_real_input_and_choices() {
    let server = Server::start("examples/INPUT.ppl");
    let mut socket = server.socket();
    let mut dialogs = vec![];
    loop {
        let packet = next(&mut socket);
        let event = &packet["event"];
        if event["kind"] == "dialog" {
            let kind = event["dialog"]["kind"].as_str().unwrap();
            dialogs.push(kind.to_owned());
            let response = match kind {
                "input" => json!({"kind":"entries","values":["7"]}),
                "choose" => json!({"kind":"choice","index":2}),
                "message" => json!({"kind":"ok"}),
                _ => panic!("unexpected dialog"),
            };
            socket.send(Message::Text(json!({"epoch":packet["epoch"],"control":{"kind":"dialog","response":response}}).to_string().into())).unwrap();
        }
        if event["kind"] == "final" {
            break;
        }
    }
    let report = server.result();
    assert_eq!(report["return_value"]["value"], "9");
    assert_eq!(dialogs, ["input", "choose", "message"]);
}

#[test]
fn browser_stop_cancels_waiting_input() {
    let server = Server::start("tests/fixtures/WAIT.ppl");
    let mut socket = server.socket();
    loop {
        let packet = next(&mut socket);
        if packet["event"]["kind"] == "frame" {
            socket
                .send(Message::Text(
                    json!({"epoch":packet["epoch"],"control":{"kind":"cancel"}})
                        .to_string()
                        .into(),
                ))
                .unwrap();
            break;
        }
    }
    let report = server.result();
    assert_eq!(report["status"], "cancelled");
    assert_eq!(report["exit_code"], 130);
}

#[test]
fn closing_the_last_viewer_cancels_blocked_execution() {
    let server = Server::start("tests/fixtures/WAIT.ppl");
    let mut socket = server.socket();
    let packet = next(&mut socket);
    assert_eq!(packet["event"]["kind"], "frame");
    socket.close(None).unwrap();
    drop(socket);
    let report = server.result();
    assert_eq!(report["status"], "cancelled");
    assert_eq!(report["exit_code"], 130);
}

#[test]
fn browser_requires_token_and_matching_origin() {
    let server = Server::start("tests/fixtures/WAIT.ppl");
    let authority = server
        .url
        .trim_start_matches("http://")
        .split('/')
        .next()
        .unwrap();
    let mut stream = TcpStream::connect(authority).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    write!(
        stream,
        "GET / HTTP/1.1\r\nHost: {authority}\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    assert!(response.starts_with("HTTP/1.1 403"));
    let mut request = server
        .url
        .replace("http:", "ws:")
        .replace("/?", "/socket?")
        .into_client_request()
        .unwrap();
    request
        .headers_mut()
        .insert("Origin", HeaderValue::from_static("http://evil.invalid"));
    assert!(tungstenite::connect(request).is_err());
    let mut socket = server.socket();
    let packet = next(&mut socket);
    socket
        .send(Message::Text(
            json!({"epoch":packet["epoch"],"control":{"kind":"cancel"}})
                .to_string()
                .into(),
        ))
        .unwrap();
    assert_eq!(server.result()["status"], "cancelled");
}
