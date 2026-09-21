use serde_json::{json, Value};
use std::{
    io::{self, BufRead, BufReader, Write},
    process::{Child, Command, Stdio},
    sync::mpsc::{self, Receiver},
    thread,
    time::{Duration, Instant},
};

/// Every receive is bounded so a protocol regression fails instead of hanging CI.
struct Client {
    process: Child,
    messages: Receiver<io::Result<Value>>,
}

impl Client {
    fn spawn() -> Self {
        let mut process = Command::new(env!("CARGO_BIN_EXE_ret"))
            .arg("server")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let stdout = process.stdout.take().unwrap();
        let (sender, messages) = mpsc::channel();
        thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                match read_message(&mut reader) {
                    Ok(Some(message)) => {
                        if sender.send(Ok(message)).is_err() {
                            break;
                        }
                    }
                    Ok(None) => break,
                    Err(error) => {
                        let _ = sender.send(Err(error));
                        break;
                    }
                }
            }
        });
        Self { process, messages }
    }

    fn send_body(&mut self, body: &[u8], extra_headers: bool) {
        let stdin = self.process.stdin.as_mut().unwrap();
        write!(stdin, "Content-Length: {}\r\n", body.len()).unwrap();
        if extra_headers {
            write!(
                stdin,
                "Content-Type: application/vscode-jsonrpc; charset=utf-8\r\n"
            )
            .unwrap();
        }
        stdin.write_all(b"\r\n").unwrap();
        stdin.write_all(body).unwrap();
        stdin.flush().unwrap();
    }

    fn send(&mut self, message: Value) {
        self.send_body(&serde_json::to_vec(&message).unwrap(), true);
    }

    fn receive(&self) -> Value {
        self.messages
            .recv_timeout(Duration::from_secs(10))
            .expect("server did not respond before the deadline")
            .expect("invalid response frame")
    }

    fn assert_exits_after_eof(&mut self) {
        drop(self.process.stdin.take());
        let started = Instant::now();
        loop {
            if let Some(status) = self.process.try_wait().unwrap() {
                assert!(status.success());
                return;
            }
            assert!(
                started.elapsed() < Duration::from_secs(10),
                "server did not exit after EOF"
            );
            thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.process.kill();
        let _ = self.process.wait();
    }
}

fn read_message(reader: &mut impl BufRead) -> io::Result<Option<Value>> {
    let mut length = None;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 {
            return Ok(None);
        }
        if line.trim().is_empty() {
            break;
        }
        if let Some(value) = line.strip_prefix("Content-Length:") {
            length = Some(
                value
                    .trim()
                    .parse::<usize>()
                    .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?,
            );
        }
    }
    let length =
        length.ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "Missing length"))?;
    let mut body = vec![0; length];
    reader.read_exact(&mut body)?;
    serde_json::from_slice(&body)
        .map(Some)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

#[test]
fn string_and_large_ids_round_trip_with_additional_headers() {
    let mut client = Client::spawn();
    for id in [
        json!("配置-α"),
        json!(4294967297u64),
        json!(-3),
        Value::Null,
    ] {
        client.send(json!({"jsonrpc": "2.0", "id": id, "method": "configure", "params": {}}));
        let reply = client.receive();
        assert_eq!(reply["id"], id);
        assert!(reply.get("error").is_none(), "{reply}");
        assert_eq!(reply["result"], Value::Null);
    }
    client.assert_exits_after_eof();
}

#[test]
fn eof_without_requests_and_eof_inside_a_frame_both_terminate() {
    Client::spawn().assert_exits_after_eof();
    let mut client = Client::spawn();
    client
        .process
        .stdin
        .as_mut()
        .unwrap()
        .write_all(b"Content-Length: 100\r\n\r\n{")
        .unwrap();
    client.assert_exits_after_eof();
}

#[test]
fn parse_errors_receive_a_response_and_do_not_corrupt_the_next_frame() {
    let mut client = Client::spawn();
    client.send_body(b"{", false);
    let error = client.receive();
    assert_eq!(error["error"]["code"], -32700);
    assert!(error["id"].is_null());
    client.send(json!({"jsonrpc": "2.0", "id": "next", "method": "configure", "params": {}}));
    let reply = client.receive();
    assert_eq!(reply["id"], "next");
    assert!(reply.get("error").is_none());
}

#[test]
fn unknown_notification_does_not_produce_a_reply() {
    let mut client = Client::spawn();
    client.send(json!({"jsonrpc": "2.0", "method": "unknown", "params": {}}));
    client.send(json!({"jsonrpc": "2.0", "id": "known", "method": "configure", "params": {}}));
    let reply = client.receive();
    assert_eq!(reply["id"], "known");
    assert!(reply.get("error").is_none());
}

#[cfg(unix)]
#[test]
fn deleted_installation_is_not_returned_by_a_long_lived_server() {
    use std::os::unix::fs::PermissionsExt;
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("installation");
    let executable = home.join("bin").join("R");
    std::fs::create_dir_all(executable.parent().unwrap()).unwrap();
    let script = format!(
        "#!/bin/sh\nprintf 'ret-r-installation-info\\n4.4.0\\n%s\\nx86_64\\n' {}\n",
        ret_core::shell::quote_shell_argument(&home.to_string_lossy())
    );
    std::fs::write(&executable, script).unwrap();
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut client = Client::spawn();
    client.send(json!({"jsonrpc": "2.0", "id": 1, "method": "resolve", "params": {"executable": executable}}));
    let first = client.receive();
    assert_eq!(first["result"]["version"], "4.4.0");
    std::fs::remove_dir_all(home).unwrap();
    client.send(json!({"jsonrpc": "2.0", "id": 2, "method": "resolve", "params": {"executable": executable}}));
    let second = client.receive();
    assert_eq!(second["id"], 2);
    assert!(
        second.get("error").is_some(),
        "deleted installation was returned: {second}"
    );
    assert!(second.get("result").is_none());
}

#[cfg(unix)]
#[test]
fn disconnect_terminates_an_active_runtime_probe() {
    use std::os::unix::fs::PermissionsExt;
    let temp = tempfile::tempdir().unwrap();
    let executable = temp.path().join("R");
    let pid_file = temp.path().join("runtime.pid");
    let script = format!(
        "#!/bin/sh\necho $$ > {}\nexec sleep 30\n",
        ret_core::shell::quote_shell_argument(&pid_file.to_string_lossy())
    );
    std::fs::write(&executable, script).unwrap();
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut client = Client::spawn();
    client.send(json!({"jsonrpc": "2.0", "id": 1, "method": "resolve", "params": {"executable": executable}}));
    let started = Instant::now();
    let pid = loop {
        if let Ok(pid) = std::fs::read_to_string(&pid_file) {
            if let Ok(pid) = pid.trim().parse::<u32>() {
                break pid;
            }
        }
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "runtime probe did not start"
        );
        thread::sleep(Duration::from_millis(10));
    };
    client.assert_exits_after_eof();
    let output = Command::new("ps")
        .args(["-o", "stat=", "-p", &pid.to_string()])
        .output()
        .unwrap();
    let state = String::from_utf8_lossy(&output.stdout);
    assert!(
        state.trim().is_empty() || state.trim().starts_with('Z'),
        "runtime remains alive: {state}"
    );
}
