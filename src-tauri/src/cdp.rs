// cdp.rs — Chrome DevTools Protocol connectivity for Publisher Rocket
//
// Launches Publisher Rocket with --remote-debugging-port=9222,
// connects via raw TCP WebSocket, exposes eval/click helpers.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::os::unix::process::CommandExt;
use std::process::Command;
use std::time::{Duration, Instant};
use serde::Deserialize;
use serde_json::{json, Value};

const CDP_HOST: &str = "127.0.0.1";
const CDP_PORT: u16 = 9222;
const ROCKET_APP: &str = "/Applications/Publisher Rocket.app";

// ── Types ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, Clone)]
pub struct PageTarget {
    pub id: String,
    pub title: String,
    #[serde(rename = "webSocketDebuggerUrl")]
    pub ws_url: String,
}

pub struct Session {
    stream: TcpStream,
    cmd_id: u32,
}

// ── Launch ────────────────────────────────────────────────────────────────────

/// Read CFBundleExecutable from Info.plist and return the full binary path.
fn resolve_mac_binary(app_path: &str) -> Result<String, String> {
    let plist = std::fs::read_to_string(format!("{}/Contents/Info.plist", app_path))
        .map_err(|e| format!("Cannot read Info.plist: {}", e))?;
    let re = regex::Regex::new(
        r"<key>CFBundleExecutable</key>\s*<string>([^<]+)</string>"
    ).unwrap();
    let exe = re.captures(&plist)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().to_string())
        .ok_or("CFBundleExecutable not found")?;
    Ok(format!("{}/Contents/MacOS/{}", app_path, exe))
}

/// Launch Publisher Rocket with --remote-debugging-port=9222.
pub fn launch_rocket() -> Result<(), String> {
    let exe = resolve_mac_binary(ROCKET_APP)?;

    let mut cmd = Command::new(&exe);
    cmd.arg(format!("--remote-debugging-port={}", CDP_PORT));

    // Scrub env vars that break Electron children
    for key in &["ELECTRON_RUN_AS_NODE", "ELECTRON_NO_ATTACH_CONSOLE",
                 "ELECTRON_NO_ASAR", "NODE_OPTIONS"] {
        cmd.env_remove(key);
    }

    // Detach — we don't own the process lifecycle
    unsafe { cmd.pre_exec(|| { libc::setsid(); Ok(()) }); }

    cmd.spawn().map_err(|e| format!("Failed to spawn Publisher Rocket: {}", e))?;

    // Give it 2s to either crash or stabilise
    std::thread::sleep(Duration::from_secs(2));
    Ok(())
}

/// Quit Publisher Rocket via AppleScript.
pub fn quit_rocket() -> Result<(), String> {
    Command::new("osascript")
        .args(["-e", r#"quit app "Publisher Rocket""#])
        .output()
        .map_err(|e| format!("Failed to quit Publisher Rocket: {}", e))?;
    Ok(())
}

// ── CDP /json polling ─────────────────────────────────────────────────────────

/// Query http://127.0.0.1:9222/json and return the Publisher Rocket target.
pub fn get_page_target() -> Result<PageTarget, String> {
    let resp = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
        .unwrap()
        .get(format!("http://{}:{}/json", CDP_HOST, CDP_PORT))
        .send()
        .map_err(|e| e.to_string())?;

    let pages: Vec<PageTarget> = resp.json().map_err(|e| e.to_string())?;

    pages.into_iter()
        .find(|p| {
            p.title == "Publisher Rocket"
                || p.title == "KDP Rocket"
                || p.title.to_lowercase().contains("rocket")
        })
        .map(|mut t| {
            // Rewrite localhost → 127.0.0.1 to avoid IPv6 trap
            t.ws_url = t.ws_url.replace(
                "ws://localhost:",
                &format!("ws://{}:", CDP_HOST),
            );
            t
        })
        .ok_or_else(|| format!("No Publisher Rocket page target on port {}", CDP_PORT))
}

/// Check if port 9222 accepts TCP connections.
pub fn is_port_open() -> bool {
    TcpStream::connect_timeout(
        &format!("{}:{}", CDP_HOST, CDP_PORT).parse().unwrap(),
        Duration::from_secs(2),
    ).is_ok()
}

/// Poll /json until a Rocket target appears or 45s elapses.
pub fn poll_for_target() -> Result<PageTarget, String> {
    let deadline = Instant::now() + Duration::from_secs(45);
    while Instant::now() < deadline {
        if let Ok(t) = get_page_target() { return Ok(t); }
        std::thread::sleep(Duration::from_millis(500));
    }
    Err(format!(
        "CDP timeout — Publisher Rocket did not expose a page target on port {} within 45s",
        CDP_PORT
    ))
}

/// Ensure Rocket is running with CDP. Handles launch / relaunch as needed.
pub fn ensure_rocket() -> Result<PageTarget, String> {
    if let Ok(t) = get_page_target() { return Ok(t); }

    if !is_port_open() {
        // Case 1: nothing on port — fresh launch
        launch_rocket()?;
        std::thread::sleep(Duration::from_secs(5));
        return poll_for_target();
    }

    // Case 2: port open but no Rocket target — quit and relaunch
    let _ = quit_rocket();
    std::thread::sleep(Duration::from_secs(2));
    launch_rocket()?;
    std::thread::sleep(Duration::from_secs(5));
    poll_for_target()
}

// ── WebSocket session ─────────────────────────────────────────────────────────

/// Connect a raw WebSocket to the CDP page endpoint.
pub fn connect(target: &PageTarget) -> Result<Session, String> {
    let mut stream = TcpStream::connect_timeout(
        &format!("{}:{}", CDP_HOST, CDP_PORT).parse().unwrap(),
        Duration::from_secs(5),
    ).map_err(|e| format!("CDP TCP connect: {}", e))?;

    let handshake = format!(
        "GET /devtools/page/{} HTTP/1.1\r\n\
         Host: {}:{}\r\n\
         Upgrade: websocket\r\n\
         Connection: Upgrade\r\n\
         Sec-WebSocket-Key: YXV0aG9ydG9vbHMxMjM0PT0=\r\n\
         Sec-WebSocket-Version: 13\r\n\r\n",
        target.id, CDP_HOST, CDP_PORT
    );
    stream.write_all(handshake.as_bytes())
        .map_err(|e| format!("Handshake write: {}", e))?;

    // Read past HTTP 101
    stream.set_read_timeout(Some(Duration::from_secs(5))).ok();
    let mut buf = [0u8; 4096];
    stream.read(&mut buf).map_err(|e| format!("Handshake read: {}", e))?;
    stream.set_read_timeout(None).ok();

    Ok(Session { stream, cmd_id: 1 })
}

// ── Send / Recv ───────────────────────────────────────────────────────────────

impl Session {
    fn send(&mut self, method: &str, params: Value) -> Result<u32, String> {
        let id = self.cmd_id;
        self.cmd_id += 1;

        let payload = serde_json::to_vec(&json!({
            "id": id, "method": method, "params": params
        })).unwrap();

        let mask = [0xfe_u8, 0xed, 0xbe, 0xef];
        let masked: Vec<u8> = payload.iter().enumerate()
            .map(|(i, b)| b ^ mask[i % 4])
            .collect();

        let len = payload.len();
        let mut frame = Vec::new();
        frame.push(0x81);
        if len <= 125 {
            frame.push(0x80 | len as u8);
        } else {
            frame.push(0x80 | 126);
            frame.push((len >> 8) as u8);
            frame.push(len as u8);
        }
        frame.extend_from_slice(&mask);
        frame.extend_from_slice(&masked);

        self.stream.write_all(&frame)
            .map_err(|e| format!("CDP send: {}", e))?;
        Ok(id)
    }

    fn recv(&mut self, target_id: u32, timeout: Duration) -> Result<Value, String> {
        self.stream.set_read_timeout(Some(timeout)).ok();
        let result = self.recv_inner(target_id);
        self.stream.set_read_timeout(None).ok();
        result
    }

    fn recv_inner(&mut self, target_id: u32) -> Result<Value, String> {
        let mut buf = Vec::new();
        loop {
            let mut tmp = [0u8; 8192];
            let n = self.stream.read(&mut tmp)
                .map_err(|e| format!("CDP recv read: {}", e))?;
            buf.extend_from_slice(&tmp[..n]);

            // Try to parse frames from buffer
            let mut pos = 0;
            while pos + 2 <= buf.len() {
                let _fin_opcode = buf[pos]; pos += 1;
                let ml = buf[pos]; pos += 1;
                let is_masked = (ml & 0x80) != 0;
                let mut plen = (ml & 0x7f) as usize;

                if plen == 126 {
                    if pos + 2 > buf.len() { break; }
                    plen = ((buf[pos] as usize) << 8) | buf[pos + 1] as usize;
                    pos += 2;
                }
                if is_masked { pos += 4; }
                if pos + plen > buf.len() { break; }

                let frame = &buf[pos..pos + plen];
                pos += plen;

                if let Ok(msg) = serde_json::from_slice::<Value>(frame) {
                    if msg.get("id").and_then(|v| v.as_u64()) == Some(target_id as u64) {
                        let val = msg["result"]["result"]["value"].clone();
                        return Ok(if val.is_null() { msg } else { val });
                    }
                }
            }
        }
    }

    /// Evaluate JavaScript in Publisher Rocket's renderer.
    pub fn eval(&mut self, expression: &str, timeout_secs: u64) -> Result<String, String> {
        let wrapped = format!("(function(){{ {} }})()", expression);
        let id = self.send("Runtime.evaluate", json!({
            "expression": wrapped,
            "returnByValue": true
        }))?;
        let result = self.recv(id, Duration::from_secs(timeout_secs))?;
        Ok(match result {
            Value::String(s) => s,
            Value::Null => String::new(),
            other => other.to_string(),
        })
    }

    /// Dispatch a mouse press+release at (x, y).
    pub fn click(&mut self, x: f64, y: f64) -> Result<(), String> {
        self.send("Input.dispatchMouseEvent", json!({
            "type": "mousePressed", "x": x, "y": y,
            "button": "left", "clickCount": 1
        }))?;
        std::thread::sleep(Duration::from_millis(80));
        self.send("Input.dispatchMouseEvent", json!({
            "type": "mouseReleased", "x": x, "y": y,
            "button": "left", "clickCount": 1
        }))?;
        std::thread::sleep(Duration::from_millis(80));
        Ok(())
    }

    /// Dispatch Escape keydown+keyup.
    pub fn key_escape(&mut self) {
        let _ = self.send("Input.dispatchKeyEvent",
            json!({"type":"keyDown","key":"Escape","code":"Escape"}));
        std::thread::sleep(Duration::from_millis(80));
        let _ = self.send("Input.dispatchKeyEvent",
            json!({"type":"keyUp","key":"Escape","code":"Escape"}));
        std::thread::sleep(Duration::from_millis(200));
    }

    /// Type a single character using CDP Input.dispatchKeyEvent.
    /// Send only the 'char' event type — keyDown+char together causes double input.
    pub fn send_char(&mut self, ch: char) {
        let text = ch.to_string();
        let _ = self.send("Input.dispatchKeyEvent", json!({
            "type": "char",
            "key":  text,
            "text": text,
            "unmodifiedText": text,
        }));
        std::thread::sleep(Duration::from_millis(30));
    }

    /// Send Ctrl+A to select all text in the focused element.
    pub fn select_all(&mut self) {
        let _ = self.send("Input.dispatchKeyEvent", json!({
            "type": "keyDown", "key": "a",
            "modifiers": 2  // Ctrl
        }));
        std::thread::sleep(Duration::from_millis(30));
        let _ = self.send("Input.dispatchKeyEvent", json!({
            "type": "keyUp", "key": "a",
            "modifiers": 2
        }));
        std::thread::sleep(Duration::from_millis(30));
    }

    /// Send Backspace.
    pub fn send_backspace(&mut self) {
        let _ = self.send("Input.dispatchKeyEvent",
            json!({"type":"keyDown","key":"Backspace","code":"Backspace"}));
        std::thread::sleep(Duration::from_millis(20));
        let _ = self.send("Input.dispatchKeyEvent",
            json!({"type":"keyUp","key":"Backspace","code":"Backspace"}));
    }

    /// Tell Electron/Chrome to save downloads silently to a specific path.
    /// Must be called before clicking any export button.
    pub fn set_download_path(&mut self, path: &str) {
        // Browser.setDownloadBehavior works at browser context level
        let _ = self.send("Browser.setDownloadBehavior", json!({
            "behavior": "allow",
            "downloadPath": path,
            "eventsEnabled": true
        }));
        std::thread::sleep(Duration::from_millis(200));
        // Also set via Page (belt-and-suspenders)
        let _ = self.send("Page.setDownloadBehavior", json!({
            "behavior": "allow",
            "downloadPath": path
        }));
        std::thread::sleep(Duration::from_millis(200));
    }
}
