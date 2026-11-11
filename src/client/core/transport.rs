//! A persistent `hc connect` transport, not a subprocess per key or RPC.
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};

use super::writer::FrameWriter;
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

pub const DATA: u8 = 0;
pub const CREDIT: u8 = 1;
pub const CLOSE: u8 = 2;
pub const RESIZE: u8 = 3;
pub const MAX_PAYLOAD: usize = 16 << 20;

#[derive(Debug)]
pub struct Frame {
    pub channel: u32,
    pub kind: u8,
    pub payload: Vec<u8>,
}

pub fn read_frame(input: &mut impl Read) -> io::Result<Frame> {
    let mut header = [0; 9];
    input.read_exact(&mut header)?;
    let channel = u32::from_le_bytes([header[0], header[1], header[2], header[3]]);
    let size = u32::from_le_bytes([header[5], header[6], header[7], header[8]]) as usize;
    if size > MAX_PAYLOAD {
        return Err(io::Error::other("Core frame exceeds 16 MiB"));
    }
    let mut payload = vec![0; size];
    input.read_exact(&mut payload)?;
    Ok(Frame {
        channel,
        kind: header[4],
        payload,
    })
}

pub struct Core {
    child: Child,
    input: FrameWriter,
    failed: bool,
    incoming: Receiver<io::Result<Frame>>,
    pending: VecDeque<Frame>,
    pending_bytes: usize,
    next_id: u64,
    pub events_channel: Option<u32>,
    pub hello: Value,
}

pub fn command(binary: &Path, home: &Path, socket: &Path) -> Command {
    let mut cmd = Command::new(binary);
    // Never inherit identity/socket overrides from the enclosing Herdr or Core.
    for (key, _) in std::env::vars_os() {
        let key_text = key.to_string_lossy();
        if key_text.starts_with("HERDR_") || key_text.starts_with("HC_") {
            cmd.env_remove(key);
        }
    }
    cmd.env("HC_SESSION", "herdr-tui")
        .env("HC_SOCKET", socket)
        // A custom runtime dir bypasses Core's systemd/linger auto-setup.
        .env("XDG_RUNTIME_DIR", home.join("run"))
        .env("XDG_STATE_HOME", home.join("state"))
        .env("XDG_CONFIG_HOME", home.join("config"));
    cmd
}

fn negotiate(input: &mut impl Write, output: &mut impl Read, existing: bool) -> io::Result<Value> {
    writeln!(
        input,
        "{}",
        json!({"jsonrpc":"2.0","id":0,"method":"hello","params":{"generation":1,"framed":true}})
    )?;
    input.flush()?;
    // Do not buffer beyond the plain reply: the next byte is a frame header.
    let mut line = Vec::new();
    loop {
        let mut byte = [0];
        output.read_exact(&mut byte)?;
        if byte[0] == b'\n' {
            break;
        }
        line.push(byte[0]);
        if line.len() > 65536 {
            return Err(io::Error::other("oversized handshake"));
        }
    }
    let reply: Value = serde_json::from_slice(&line)?;
    let result = rpc_result(&reply)?;
    if result["generation"] != 1 || result["framed"] != true {
        return Err(io::Error::other(
            "Core did not negotiate generation 1 framed mode",
        ));
    }
    let features = result["features"]
        .as_array()
        .ok_or_else(|| io::Error::other("missing features"))?;
    for feature in [
        "attach",
        "events",
        "status",
        "resize_notifications",
        "automatic_input",
        "ordered_attach",
        "keyboard_stacks_v1",
    ] {
        if !features.iter().any(|v| v == feature) {
            return Err(io::Error::other(format!("Core lacks {feature}; build the pinned runtime with just core-source-build and set HERDR_CORE_BIN to its binary. Running servers are never replaced automatically; use a separate HERDR_CORE_HOME or plan an explicit upgrade")));
        }
    }
    if existing && !features.iter().any(|v| v == "existing_connect") {
        // A probe without this capability could auto-start a stopped runtime.
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Core lacks existing_connect; refusing a recovery probe that might start a stopped runtime",
        ));
    }

    let methods = result["methods"]
        .as_array()
        .ok_or_else(|| io::Error::other("missing Core methods"))?;
    for method in [
        "events.subscribe",
        "terminal.run",
        "terminal.attach_ordered",
        "terminal.kill",
        "terminal.get",
        "terminal.read",
        "doc.get",
        "doc.set",
    ] {
        if !methods.iter().any(|value| value == method) {
            return Err(io::Error::other(format!(
                "Core lacks required method {method}"
            )));
        }
    }
    Ok(result)
}

impl Core {
    /// Explicit launch: may start a Core that is not running yet.
    pub fn connect(binary: &Path, home: &Path, socket: &Path) -> io::Result<Self> {
        Self::open(binary, home, socket, false)
    }

    /// Recovery probe: reaches only a running Core. A probe must never undo an
    /// explicit `server stop` by auto-starting a replacement runtime.
    pub fn connect_existing(binary: &Path, home: &Path, socket: &Path) -> io::Result<Self> {
        Self::open(binary, home, socket, true)
    }

    fn open(binary: &Path, home: &Path, socket: &Path, existing: bool) -> io::Result<Self> {
        std::fs::create_dir_all(home.join("run"))?;
        let mut command = command(binary, home, socket);
        command.arg("connect");
        if existing {
            command.arg("--existing");
        }
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()?;
        let handshake = (|| -> io::Result<_> {
            let mut input = child
                .stdin
                .take()
                .ok_or_else(|| io::Error::other("missing stdin"))?;
            let mut output = child
                .stdout
                .take()
                .ok_or_else(|| io::Error::other("missing stdout"))?;
            let (handshake_tx, handshake_rx) = mpsc::channel();
            std::thread::Builder::new()
                .name("core-handshake".into())
                .spawn(move || {
                    let reply = negotiate(&mut input, &mut output, existing)
                        .map(|hello| (input, output, hello));
                    let _ = handshake_tx.send(reply);
                })?;
            handshake_rx
                .recv_timeout(Duration::from_secs(20))
                .map_err(|error| io::Error::other(format!("Core handshake failed: {error}")))?
        })();
        let (input, mut output, hello) = match handshake {
            Ok(parts) => parts,
            Err(err) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(err);
            }
        };
        let input = match FrameWriter::new(input) {
            Ok(writer) => writer,
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error);
            }
        };
        // Bounded: a paused UI cannot turn a busy terminal into unbounded memory.
        let (tx, incoming) = mpsc::sync_channel(4);
        std::thread::spawn(move || loop {
            let frame = read_frame(&mut output);
            let failed = frame.is_err();
            if tx.send(frame).is_err() || failed {
                break;
            }
        });
        Ok(Self {
            child,
            input,
            failed: false,
            incoming,
            pending: VecDeque::new(),
            pending_bytes: 0,
            next_id: 1,
            events_channel: None,
            hello,
        })
    }

    pub fn send(&mut self, channel: u32, kind: u8, payload: &[u8]) -> io::Result<()> {
        self.send_with_timeout(channel, kind, payload, Duration::from_secs(5))
    }

    fn send_with_timeout(
        &mut self,
        channel: u32,
        kind: u8,
        payload: &[u8],
        timeout: Duration,
    ) -> io::Result<()> {
        if payload.len() > MAX_PAYLOAD {
            return Err(io::Error::other("outgoing frame too large"));
        }
        self.check_connection()?;
        let result = self.input.send(channel, kind, payload, timeout);
        self.failed |= result.is_err();
        result
    }

    pub fn credit(&mut self, frame: &Frame) -> io::Result<()> {
        if frame.channel != 0 && frame.kind == DATA {
            self.check_connection()?;
            let result = self.input.notify_u32(
                frame.channel,
                CREDIT,
                frame.payload.len() as u32,
                Duration::from_secs(5),
            );
            self.failed |= result.is_err();
            result?;
        }
        Ok(())
    }

    fn check_connection(&mut self) -> io::Result<()> {
        if self.failed {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "Core connection failed; pending input/actions were not retried",
            ));
        }
        let result = self.input.health();
        self.failed |= result.is_err();
        result
    }

    pub fn request(&mut self, method: &str, params: Value) -> io::Result<Value> {
        self.request_with_timeout(method, params, Duration::from_secs(15))
    }

    fn request_with_timeout(
        &mut self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> io::Result<Value> {
        let deadline = Instant::now() + timeout;
        self.check_connection()?;
        if !self.hello["methods"]
            .as_array()
            .is_some_and(|methods| methods.iter().any(|value| value == method))
        {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                format!("Core does not advertise {method}"),
            ));
        }
        let id = self.next_id;
        self.next_id += 1;
        self.send_with_timeout(
            0,
            DATA,
            &serde_json::to_vec(&json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}))?,
            deadline
                .saturating_duration_since(Instant::now())
                .min(Duration::from_secs(5)),
        )?;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let received =
                self.incoming
                    .recv_timeout(remaining)
                    .map_err(|error| {
                        io::Error::new(match error {
                    mpsc::RecvTimeoutError::Timeout => io::ErrorKind::TimedOut,
                    mpsc::RecvTimeoutError::Disconnected => io::ErrorKind::BrokenPipe,
                }, "Core RPC interrupted; outcome may be committed and will not be replayed")
                    })
                    .and_then(|frame| frame);
            let frame = match received {
                Ok(frame) => frame,
                Err(error) => {
                    self.failed = true;
                    return Err(error);
                }
            };
            if frame.channel == 0 && frame.kind == DATA {
                let reply: Value = match serde_json::from_slice(&frame.payload) {
                    Ok(reply) => reply,
                    Err(error) => {
                        self.failed = true;
                        return Err(io::Error::new(io::ErrorKind::InvalidData, error));
                    }
                };
                if reply["id"].as_u64() == Some(id) {
                    return rpc_result(&reply);
                }
            }
            // Keep credit withheld until the UI consumes deferred terminal bytes.
            if self.pending.len() >= 256
                || self.pending_bytes.saturating_add(frame.payload.len()) > MAX_PAYLOAD
            {
                self.failed = true;
                return Err(io::Error::other("too many deferred frames during RPC"));
            }
            self.pending_bytes += frame.payload.len();
            self.pending.push_back(frame);
        }
    }

    pub fn subscribe(&mut self) -> io::Result<Value> {
        let result = self.request("events.subscribe", json!({}))?;
        self.events_channel = result["channel"]
            .as_u64()
            .and_then(|n| u32::try_from(n).ok());
        if self.events_channel.is_none() {
            return Err(io::Error::other("missing event channel"));
        }
        Ok(result)
    }

    pub fn disconnect(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }

    pub fn poll(&mut self) -> io::Result<Option<Frame>> {
        self.check_connection()?;
        if let Some(frame) = self.pending.pop_front() {
            self.pending_bytes -= frame.payload.len();
            return Ok(Some(frame));
        }
        match self.incoming.try_recv() {
            Ok(Ok(frame)) => Ok(Some(frame)),
            Ok(Err(error)) => {
                self.failed = true;
                Err(error)
            }
            Err(mpsc::TryRecvError::Empty) => Ok(None),
            Err(mpsc::TryRecvError::Disconnected) => {
                self.failed = true;
                Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "Core transport disconnected",
                ))
            }
        }
    }
}

impl Drop for Core {
    fn drop(&mut self) {
        // Ends only this stdio relay. The server, holders and programs stay alive.
        self.disconnect();
    }
}

fn rpc_result(reply: &Value) -> io::Result<Value> {
    if let Some(error) = reply.get("error") {
        let kind = match error["data"]["kind"].as_str() {
            Some("conflict") => io::ErrorKind::WouldBlock,
            Some("not_found" | "unavailable") => io::ErrorKind::NotFound,
            _ => io::ErrorKind::Other,
        };
        return Err(io::Error::new(kind, error.to_string()));
    }
    reply
        .get("result")
        .cloned()
        .ok_or_else(|| io::Error::other("missing RPC result"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_spawned_core_connects_to_the_configured_socket() {
        let home = Path::new("/home/state/core-client-v1");
        let socket = Path::new("/run/user/1000/herdr-core/default.sock");
        let command = command(Path::new("herdr-core"), home, socket);
        let envs = command
            .get_envs()
            .collect::<std::collections::HashMap<_, _>>();

        assert_eq!(
            envs.get(std::ffi::OsStr::new("HC_SOCKET"))
                .and_then(|value| value.map(std::path::PathBuf::from)),
            Some(socket.to_path_buf()),
            "the child must join the configured Core instead of <home>/core.sock"
        );
        assert_eq!(
            envs.get(std::ffi::OsStr::new("XDG_RUNTIME_DIR"))
                .and_then(|value| value.map(std::path::PathBuf::from)),
            Some(home.join("run")),
            "client-local runtime state still follows the home directory"
        );
    }
    fn check_with(result: Value, existing: bool) -> io::Result<Value> {
        let mut bytes =
            serde_json::to_vec(&json!({"jsonrpc":"2.0","id":0,"result":result})).unwrap();
        bytes.push(b'\n');
        let mut sent = Vec::new();
        let answer = negotiate(&mut sent, &mut bytes.as_slice(), existing);
        assert!(String::from_utf8(sent).unwrap().contains("hello"));
        answer
    }

    #[test]
    fn a_recovery_probe_requires_existing_connect_and_launches_do_not() {
        let result = json!({"generation":1,"framed":true,"features":["attach","events","status","resize_notifications","automatic_input","ordered_attach","keyboard_stacks_v1"],"methods":["events.subscribe","terminal.run","terminal.attach_ordered","terminal.kill","terminal.get","terminal.read","doc.get","doc.set"]});
        assert!(check_with(result.clone(), false).is_ok());
        let error = check_with(result.clone(), true).expect_err("probe must be refused");
        assert_eq!(error.kind(), io::ErrorKind::Unsupported);
        assert!(error.to_string().contains("existing_connect"), "{error}");

        let mut probe = result;
        probe["features"]
            .as_array_mut()
            .unwrap()
            .push("existing_connect".into());
        assert!(check_with(probe, true).is_ok());
    }

    #[test]
    fn hello_rejects_missing_features_and_required_methods() {
        let result = json!({"generation":1,"framed":true,"features":["attach","events","status","resize_notifications","automatic_input","ordered_attach","keyboard_stacks_v1"],"methods":["events.subscribe","terminal.run","terminal.attach_ordered","terminal.kill","terminal.get","terminal.read","doc.get","doc.set"]});
        let check = |result: Value| check_with(result, false);
        assert!(check(result.clone()).is_ok());
        for field in ["features", "methods"] {
            for value in result[field].as_array().unwrap() {
                let mut missing = result.clone();
                missing[field]
                    .as_array_mut()
                    .unwrap()
                    .retain(|entry| entry != value);
                let error = check(missing).unwrap_err().to_string();
                assert!(error.contains(value.as_str().unwrap()), "{error}");
            }
        }
        let mut future = result;
        future["generation"] = json!(2);
        assert!(check(future).is_err());
    }

    #[test]
    fn frame_decode_preserves_binary_bytes_and_rejects_oversize_and_truncation() {
        let mut bytes = Vec::from(7u32.to_le_bytes());
        bytes.push(DATA);
        bytes.extend(3u32.to_le_bytes());
        bytes.extend([0, 0xff, 0x1b]);
        let f = read_frame(&mut bytes.as_slice()).unwrap();
        assert_eq!(
            (f.channel, f.kind, f.payload),
            (7, DATA, vec![0, 0xff, 0x1b])
        );
        assert!(read_frame(&mut &bytes[..10]).is_err());
        bytes[5..9].copy_from_slice(&((MAX_PAYLOAD + 1) as u32).to_le_bytes());
        assert!(read_frame(&mut bytes.as_slice()).is_err());
    }
    fn test_core(incoming: Receiver<io::Result<Frame>>) -> Core {
        let child = crate::noninteractive_process::command(std::env::current_exe().unwrap())
            .arg("--list")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        Core {
            child,
            input: FrameWriter::new(io::sink()).unwrap(),
            failed: false,
            incoming,
            pending: VecDeque::new(),
            pending_bytes: 0,
            next_id: 1,
            events_channel: None,
            hello: json!({"methods":["doc.set"]}),
        }
    }

    #[test]
    fn rpc_timeout_fences_late_replies_and_does_not_retry_a_possibly_committed_action() {
        let (send, receive) = mpsc::channel();
        let mut core = test_core(receive);
        assert_eq!(
            core.request_with_timeout("doc.set", json!({}), Duration::from_millis(20))
                .unwrap_err()
                .kind(),
            io::ErrorKind::TimedOut
        );
        assert!(core.failed);
        send.send(Ok(Frame {
            channel: 0,
            kind: DATA,
            payload: serde_json::to_vec(&json!({"id":1,"result":{"version":2}})).unwrap(),
        }))
        .unwrap();
        assert_eq!(
            core.request("doc.set", json!({})).unwrap_err().kind(),
            io::ErrorKind::BrokenPipe
        );
        assert_eq!(
            core.next_id, 2,
            "the second request never entered the old connection"
        );
        assert!(
            core.poll().is_err(),
            "old replies cannot leak into a new epoch"
        );
    }

    #[test]
    fn rpc_rejection_does_not_poison_a_healthy_transport() {
        let (send, receive) = mpsc::channel();
        let mut core = test_core(receive);
        for (id, reply) in [
            (
                1,
                json!({"error":{"code":-32000,"data":{"kind":"conflict"}}}),
            ),
            (2, json!({"result":{"version":3}})),
        ] {
            let mut reply = reply;
            reply["id"] = json!(id);
            send.send(Ok(Frame {
                channel: 0,
                kind: DATA,
                payload: serde_json::to_vec(&reply).unwrap(),
            }))
            .unwrap();
        }
        assert_eq!(
            core.request("doc.set", json!({})).unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
        assert!(!core.failed);
        assert_eq!(core.request("doc.set", json!({})).unwrap()["version"], 3);
    }

    #[test]
    fn reader_failure_fences_deferred_old_frames() {
        let (send, receive) = mpsc::channel();
        let mut core = test_core(receive);
        send.send(Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "test relay ended",
        )))
        .unwrap();
        assert_eq!(
            core.poll().unwrap_err().kind(),
            io::ErrorKind::UnexpectedEof
        );
        core.pending.push_back(Frame {
            channel: 7,
            kind: DATA,
            payload: b"old data".to_vec(),
        });
        assert_eq!(core.poll().unwrap_err().kind(), io::ErrorKind::BrokenPipe);
    }

    #[test]
    fn rpc_errors_are_not_treated_as_success() {
        assert!(rpc_result(&json!({"error":{"message":"blocked"}})).is_err());
        assert!(rpc_result(&json!({})).is_err());
        assert_eq!(
            rpc_result(&json!({"error":{"code":-32000,"data":{"kind":"conflict"}}}))
                .unwrap_err()
                .kind(),
            io::ErrorKind::WouldBlock
        );
    }
}
