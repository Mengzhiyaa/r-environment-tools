// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::{
    fs::File,
    io::{self, Read, Seek, SeekFrom},
    process::{Child, Command, Output, Stdio},
    sync::{Arc, Mutex, Weak},
    thread,
    time::{Duration, Instant},
};

/// A probe must not wait indefinitely for an R runtime or package manager.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_OUTPUT_BYTES: u64 = 16 * 1024 * 1024;
const POLL_INTERVAL: Duration = Duration::from_millis(10);

struct ProbeRegistry {
    stopped: bool,
    children: Vec<Weak<Mutex<ProbeChild>>>,
}

static PROBES: Mutex<ProbeRegistry> = Mutex::new(ProbeRegistry {
    stopped: false,
    children: Vec::new(),
});

/// Permanently stop probing in this process and reap active probes before the
/// stdio server exits. Registering a child and stopping the registry are atomic.
pub fn shutdown_probes() {
    let children = {
        let mut registry = PROBES.lock().expect("probe registry mutex poisoned");
        registry.stopped = true;
        std::mem::take(&mut registry.children)
            .into_iter()
            .filter_map(|child| child.upgrade())
            .collect::<Vec<_>>()
    };
    for child in children {
        child
            .lock()
            .expect("probe child mutex poisoned")
            .terminate();
    }
}

fn spawn_probe(command: &mut Command) -> io::Result<Arc<Mutex<ProbeChild>>> {
    let mut registry = PROBES.lock().expect("probe registry mutex poisoned");
    if registry.stopped {
        return Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "Probe service is shutting down",
        ));
    }
    let child = Arc::new(Mutex::new(ProbeChild(command.spawn()?)));
    registry.children.retain(|child| child.strong_count() > 0);
    registry.children.push(Arc::downgrade(&child));
    Ok(child)
}

pub fn probe_output(command: &mut Command) -> io::Result<Output> {
    output_with_timeout(command, PROBE_TIMEOUT)
}

/// Capture output without pipe-reader threads that could outlive the deadline
/// when a descendant inherits stdout/stderr. Temporary files are removed on drop.
pub fn output_with_timeout(command: &mut Command, timeout: Duration) -> io::Result<Output> {
    let started = Instant::now();
    let mut stdout = tempfile::tempfile()?;
    let mut stderr = tempfile::tempfile()?;
    command
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout.try_clone()?))
        .stderr(Stdio::from(stderr.try_clone()?));
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let spawned = spawn_probe(command);
    // Command retains its configured file handles; do not retain our temp files
    // if the caller reuses the command after this probe.
    command.stdout(Stdio::null()).stderr(Stdio::null());
    let process = spawned?;
    loop {
        if stdout.metadata()?.len() > MAX_OUTPUT_BYTES
            || stderr.metadata()?.len() > MAX_OUTPUT_BYTES
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Probe output exceeds size limit",
            ));
        }
        let status = process
            .lock()
            .expect("probe child mutex poisoned")
            .0
            .try_wait()?;
        if let Some(status) = status {
            return Ok(Output {
                status,
                stdout: read_output(&mut stdout)?,
                stderr: read_output(&mut stderr)?,
            });
        }
        let remaining = timeout.saturating_sub(started.elapsed());
        if remaining.is_zero() {
            return Err(io::Error::new(io::ErrorKind::TimedOut, "Probe timed out"));
        }
        thread::sleep(POLL_INTERVAL.min(remaining));
    }
}

fn read_output(file: &mut File) -> io::Result<Vec<u8>> {
    file.seek(SeekFrom::Start(0))?;
    let mut bytes = Vec::new();
    file.take(MAX_OUTPUT_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_OUTPUT_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Probe output exceeds size limit",
        ));
    }
    Ok(bytes)
}

struct ProbeChild(Child);

impl ProbeChild {
    fn terminate(&mut self) {
        if matches!(self.0.try_wait(), Ok(Some(_))) {
            return;
        }
        #[cfg(unix)]
        {
            // The child leads the dedicated process group configured above.
            // Kill the group so shell wrappers do not leave their R child behind.
            unsafe {
                libc::kill(-(self.0.id() as libc::pid_t), libc::SIGKILL);
            }
        }
        #[cfg(windows)]
        {
            // taskkill handles descendants of cmd.exe and other wrappers.
            // Bound the cleanup command itself and fall back to killing the root.
            if let Ok(mut killer) = crate::executable::new_silent_command("taskkill")
                .args(["/PID", &self.0.id().to_string(), "/T", "/F"])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
            {
                let started = Instant::now();
                while matches!(killer.try_wait(), Ok(None))
                    && started.elapsed() < Duration::from_secs(1)
                {
                    thread::sleep(POLL_INTERVAL);
                }
                let _ = killer.kill();
                let _ = killer.wait();
            }
        }
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

impl Drop for ProbeChild {
    fn drop(&mut self) {
        self.terminate();
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn captures_both_streams_and_preserves_exit_status() {
        let output = output_with_timeout(
            Command::new("sh").args(["-c", "printf hello; printf problem >&2; exit 7"]),
            Duration::from_secs(5),
        )
        .unwrap();
        assert_eq!(output.stdout, b"hello");
        assert_eq!(output.stderr, b"problem");
        assert_eq!(output.status.code(), Some(7));
    }

    #[test]
    fn terminates_a_hanging_probe_and_its_shell_child() {
        let temp = tempfile::tempdir().unwrap();
        let pid_file = temp.path().join("child.pid");
        let started = Instant::now();
        let error = output_with_timeout(
            Command::new("sh")
                .args(["-c", "sleep 30 & echo $! > \"$1\"; wait", "probe"])
                .arg(&pid_file),
            Duration::from_millis(500),
        )
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert!(started.elapsed() < Duration::from_secs(5));
        let pid: i32 = std::fs::read_to_string(pid_file)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        // kill -0 can see a terminated zombie; ps distinguishes it from a live child.
        let cleanup_started = Instant::now();
        loop {
            let output = Command::new("ps")
                .args(["-o", "stat=", "-p", &pid.to_string()])
                .output()
                .unwrap();
            let state = String::from_utf8_lossy(&output.stdout);
            if state.trim().is_empty() || state.trim().starts_with('Z') {
                break;
            }
            assert!(
                cleanup_started.elapsed() < Duration::from_secs(5),
                "child is still alive: {state}"
            );
            thread::sleep(POLL_INTERVAL);
        }
    }

    #[test]
    fn inherited_output_handles_do_not_delay_completion() {
        // A successful probe must not wait for a background descendant's handles.
        let output = output_with_timeout(
            Command::new("sh").args(["-c", "sleep 30 & printf '%s' \"$!\""]),
            Duration::from_secs(5),
        )
        .unwrap();
        let pid: i32 = String::from_utf8(output.stdout).unwrap().parse().unwrap();
        // Clean up this deliberately detached test child.
        unsafe {
            libc::kill(pid, libc::SIGKILL);
        }
        assert!(output.status.success());
    }

    #[test]
    fn closes_stdin_and_rejects_excessive_output() {
        let output = output_with_timeout(
            Command::new("sh").args(["-c", "cat; printf done"]),
            Duration::from_secs(5),
        )
        .unwrap();
        assert_eq!(output.stdout, b"done");
        let error = output_with_timeout(
            Command::new("sh").args(["-c", "head -c 16777217 /dev/zero"]),
            Duration::from_secs(5),
        )
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }
}

#[cfg(all(test, windows))]
mod windows_tests {
    use super::*;
    use crate::executable::new_silent_command;

    #[test]
    fn captures_windows_probe_output_and_failure_status() {
        let output = output_with_timeout(
            new_silent_command("cmd").args(["/C", "echo output& echo problem 1>&2& exit /b 7"]),
            Duration::from_secs(5),
        )
        .unwrap();
        assert!(String::from_utf8_lossy(&output.stdout).contains("output"));
        assert!(String::from_utf8_lossy(&output.stderr).contains("problem"));
        assert_eq!(output.status.code(), Some(7));
    }

    #[test]
    fn hanging_windows_wrapper_is_terminated() {
        let started = Instant::now();
        let error = output_with_timeout(
            new_silent_command("cmd").args(["/C", "ping -n 30 127.0.0.1 > nul"]),
            Duration::from_millis(500),
        )
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert!(started.elapsed() < Duration::from_secs(5));
    }
}
