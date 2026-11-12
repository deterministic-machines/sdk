#![allow(dead_code)]

use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

use dvm_api::ByteChunk;
use serde_json::Value;

pub struct Session {
    pub child: Child,
    input: Option<ChildStdin>,
    pending: Vec<u8>,
    answers: VecDeque<Value>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    deadline: Instant,
    host: bool,
}

impl Session {
    pub fn host() -> Self {
        Self::host_with_stdin(Stdio::piped())
    }

    pub fn host_with_stdin(stdin: impl Into<Stdio>) -> Self {
        let mut session = Self::start_with_stdin(
            Command::new(env!("CARGO_BIN_EXE_sandbox-server")),
            Duration::from_secs(4),
            stdin,
        );
        session.host = true;
        session
    }

    pub fn start(command: Command, limit: Duration) -> Self {
        Self::start_with_stdin(command, limit, Stdio::piped())
    }

    fn start_with_stdin(mut command: Command, limit: Duration, stdin: impl Into<Stdio>) -> Self {
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
            pending: Vec::new(),
            answers: VecDeque::new(),
            stdout: Vec::new(),
            stderr: Vec::new(),
            deadline: Instant::now() + limit,
            host: false,
        }
    }

    pub fn send(&mut self, request: Value) {
        let mut line = serde_json::to_vec(&request).unwrap();
        line.push(b'\n');
        self.raw(&line);
    }

    pub fn raw(&mut self, mut bytes: &[u8]) {
        while !bytes.is_empty() {
            self.check_deadline("writing requests");
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

    pub fn next(&mut self) -> Value {
        loop {
            if let Some(answer) = self.answers.pop_front() {
                return answer;
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
            self.check_deadline("waiting for process exit");
            if let Some(status) = self.child.try_wait().unwrap() {
                self.read_output();
                return status;
            }
            self.pump(false);
        }
    }

    pub fn wait_until(&mut self, operation: &str, mut condition: impl FnMut() -> bool) {
        loop {
            self.check_deadline(operation);
            if condition() {
                return;
            }
            self.pump(false);
        }
    }

    fn pump(&mut self, writing: bool) {
        self.check_deadline("waiting for an answer or exit");
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
                self.check_deadline("reading process output");
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
                        self.pending.extend_from_slice(&bytes[..count]);
                        while let Some(newline) =
                            self.pending.iter().position(|&byte| byte == b'\n')
                        {
                            let line = self.pending.drain(..=newline).collect::<Vec<_>>();
                            let answer = serde_json::from_slice(&line).unwrap_or_else(|error| {
                                panic!(
                                    "non-answer stdout: {error}: {}",
                                    String::from_utf8_lossy(&line)
                                )
                            });
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

    fn check_deadline(&self, operation: &str) {
        assert!(
            Instant::now() < self.deadline,
            "bounded wait expired {operation}; {}",
            self.diagnostics()
        );
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

pub fn encoded(bytes: &[u8]) -> Value {
    serde_json::to_value(ByteChunk::new(bytes.to_vec())).unwrap()
}

pub fn output(answers: &[Value], field: &str) -> Vec<u8> {
    answers
        .iter()
        .filter_map(|answer| answer.get(field))
        .flat_map(|value| {
            serde_json::from_value::<ByteChunk>(value.clone())
                .unwrap()
                .bytes()
                .to_vec()
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
