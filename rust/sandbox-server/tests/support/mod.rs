#![allow(dead_code)]

use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};

use dvm_api::{FrameDecoder, encode_frame};
use serde_json::Value;

pub struct Session {
    pub child: Child,
    input: Option<ChildStdin>,
    decoder: FrameDecoder,
    answers: VecDeque<Value>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    host: bool,
}

impl Session {
    pub fn host() -> Self {
        Self::host_with_stdin(Stdio::piped())
    }

    pub fn host_with_stdin(stdin: impl Into<Stdio>) -> Self {
        let mut session =
            Self::start_with_stdin(Command::new(env!("CARGO_BIN_EXE_sandbox-server")), stdin);
        session.host = true;
        session
    }

    pub fn start(command: Command) -> Self {
        Self::start_with_stdin(command, Stdio::piped())
    }

    fn start_with_stdin(mut command: Command, stdin: impl Into<Stdio>) -> Self {
        let mut child = command
            .stdin(stdin)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        for fd in child
            .stdin
            .as_ref()
            .map(AsRawFd::as_raw_fd)
            .into_iter()
            .chain([
                child.stdout.as_ref().unwrap().as_raw_fd(),
                child.stderr.as_ref().unwrap().as_raw_fd(),
            ])
        {
            let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
            assert!(flags >= 0);
            assert_eq!(
                unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) },
                0
            );
        }
        let input = child.stdin.take();
        Self {
            child,
            input,
            decoder: FrameDecoder::default(),
            answers: VecDeque::new(),
            stdout: Vec::new(),
            stderr: Vec::new(),
            host: false,
        }
    }

    pub fn send(&mut self, request: Value) {
        self.send_bytes(request, &[]);
    }

    pub fn send_bytes(&mut self, request: Value, payload: &[u8]) {
        self.raw(&encode_frame(&request, payload));
    }

    pub fn raw(&mut self, mut bytes: &[u8]) {
        while !bytes.is_empty() {
            match self.input.as_mut().unwrap().write(bytes) {
                Ok(count) => {
                    assert_ne!(count, 0);
                    bytes = &bytes[count..];
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => self.pump(true),
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) => panic!("request write: {error}; {}", self.diagnostics()),
            }
        }
    }

    /// The next answer; it fails the test once the process exited without it.
    pub fn next(&mut self) -> Value {
        loop {
            if let Some(answer) = self.answers.pop_front() {
                return answer;
            }
            if self.child.try_wait().unwrap().is_some() {
                self.read_output();
                return self.answers.pop_front().unwrap_or_else(|| {
                    panic!("it exited without answering; {}", self.diagnostics())
                });
            }
            self.pump(false);
        }
    }

    pub fn through_exit(&mut self, id: u64) -> Vec<Value> {
        let mut answers = Vec::new();
        loop {
            let answer = self.next();
            assert_eq!(answer["id"], id, "{answer}");
            let exited = answer.get("exited").is_some();
            answers.push(answer);
            if exited {
                return answers;
            }
        }
    }

    pub fn close(&mut self) {
        self.input = None;
    }

    pub fn finish(&mut self) -> ExitStatus {
        self.close();
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                self.read_output();
                return status;
            }
            self.pump(false);
        }
    }

    /// Waits until `condition` holds, checked again as the process writes and every poll's
    /// 20 ms.
    pub fn wait_until(&mut self, mut condition: impl FnMut() -> bool) {
        while !condition() {
            self.pump(false);
        }
    }

    fn pump(&mut self, writing: bool) {
        let mut fds = vec![
            libc::pollfd {
                fd: self.child.stdout.as_ref().unwrap().as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: self.child.stderr.as_ref().unwrap().as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
        ];
        if writing {
            fds.push(libc::pollfd {
                fd: self.input.as_ref().unwrap().as_raw_fd(),
                events: libc::POLLOUT,
                revents: 0,
            });
        }
        let result = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, 20) };
        assert!(result >= 0 || io::Error::last_os_error().kind() == io::ErrorKind::Interrupted);
        self.read_output();
    }

    fn read_output(&mut self) {
        let mut bytes = [0; 16384];
        for stderr in [false, true] {
            loop {
                let result = if stderr {
                    self.child.stderr.as_mut().unwrap().read(&mut bytes)
                } else {
                    self.child.stdout.as_mut().unwrap().read(&mut bytes)
                };
                match result {
                    Ok(0) => break,
                    Ok(count) if stderr => self.stderr.extend_from_slice(&bytes[..count]),
                    Ok(count) => {
                        self.stdout.extend_from_slice(&bytes[..count]);
                        self.decoder.push(&bytes[..count]);
                        while let Some(frame) = self.decoder.next_frame::<Value>() {
                            let frame = frame.expect("answer frame");
                            let mut answer = frame.header;
                            if !frame.payload.is_empty() {
                                answer["payload"] = serde_json::json!(frame.payload);
                            }
                            self.answers.push_back(answer);
                        }
                    }
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                    Err(error) => panic!("reading output: {error}"),
                }
            }
        }
    }

    pub fn diagnostics(&self) -> String {
        format!(
            "stdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&self.stdout),
            String::from_utf8_lossy(&self.stderr)
        )
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        if self.host && !std::thread::panicking() {
            assert!(self.finish().success(), "{}", self.diagnostics());
            assert!(self.stderr.is_empty(), "{}", self.diagnostics());
        }
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

pub fn output(answers: &[Value], field: &str) -> Vec<u8> {
    answers
        .iter()
        .filter(|answer| answer.get(field).is_some())
        .flat_map(|answer| {
            answer["payload"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_u64().unwrap() as u8)
        })
        .collect()
}

pub fn ordered(answers: &[Value], code: i32) {
    assert!(
        answers.first().unwrap().get("started").is_some(),
        "{answers:?}"
    );
    assert_eq!(
        answers.last().unwrap()["exited"]["code"],
        code,
        "{answers:?}"
    );
    for answer in &answers[1..answers.len() - 1] {
        assert!(
            answer.get("stdout").is_some() || answer.get("stderr").is_some(),
            "{answer}"
        );
    }
}
