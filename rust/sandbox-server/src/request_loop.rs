use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};

use dvm_api::{SandboxAnswer, SandboxAnswerKind, SandboxRequest, SandboxRequestAction};

use crate::commands::Commands;
use crate::files::Files;

pub struct Poller(OwnedFd);

pub struct Answers {
    buffer: Vec<u8>,
}

struct Requests {
    line: Vec<u8>,
    too_long: bool,
    last_id: Option<u64>,
    files: Files,
}

pub fn serve() -> io::Result<()> {
    let signals = signal_fd()?;
    let poller = Poller(descriptor(unsafe {
        libc::epoll_create1(libc::EPOLL_CLOEXEC)
    })?);
    nonblocking(0)?;
    let stdin_watched = poller.watch(0, libc::EPOLLIN)?;
    poller.watch(signals.as_raw_fd(), libc::EPOLLIN)?;
    let mut commands = Commands::new()?;
    let mut answers = Answers { buffer: Vec::new() };
    let mut requests = Requests {
        line: Vec::new(),
        too_long: false,
        last_id: None,
        files: Files::new(),
    };
    let result = (|| {
        let mut timeout = if stdin_watched { -1 } else { 0 };
        let mut stopping = false;
        loop {
            if !stdin_watched && !stopping {
                stopping |= requests.read(&mut commands, &poller, &mut answers)?;
            }
            let mut event = libc::epoll_event { events: 0, u64: 0 };
            let count = unsafe { libc::epoll_wait(poller.0.as_raw_fd(), &mut event, 1, timeout) };
            if count < 0 {
                if io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(io::Error::last_os_error());
            }
            if count > 0 {
                let fd = event.u64 as RawFd;
                if fd == 0 {
                    stopping |= requests.read(&mut commands, &poller, &mut answers)?;
                } else if fd == signals.as_raw_fd() {
                    stopping |= take_signals(fd)?;
                } else {
                    commands.ready(fd, &poller, &mut answers)?;
                }
            }
            let reading_file = requests.files.advance_reads(&mut answers);
            commands.reap(&mut answers)?;
            if !answers.flush()? || stopping {
                return Ok(());
            }
            timeout = if reading_file || !stdin_watched {
                0
            } else {
                -1
            };
        }
    })();
    commands.shutdown();
    result
}

impl Requests {
    fn read(
        &mut self,
        commands: &mut Commands,
        poller: &Poller,
        answers: &mut Answers,
    ) -> io::Result<bool> {
        let mut bytes = [0; 8192];
        let count = unsafe { libc::read(0, bytes.as_mut_ptr().cast(), bytes.len()) };
        if count < 0 {
            return match io::Error::last_os_error().kind() {
                io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted => Ok(false),
                _ => Err(io::Error::last_os_error()),
            };
        }
        if count == 0 {
            if !self.line.is_empty() || self.too_long {
                self.error(None, "unterminated request line", answers);
            }
            return Ok(true);
        }
        for &byte in &bytes[..count as usize] {
            if byte == b'\n' {
                if self.too_long {
                    self.error(None, "request line exceeds 65536 bytes", answers);
                } else {
                    self.dispatch(commands, poller, answers);
                }
                self.line.clear();
                self.too_long = false;
            } else if self.line.len() == 65536 {
                self.too_long = true;
            } else if !self.too_long {
                self.line.push(byte);
            }
        }
        Ok(false)
    }

    fn dispatch(&mut self, commands: &mut Commands, poller: &Poller, answers: &mut Answers) {
        let request = match serde_json::from_slice::<SandboxRequest>(&self.line) {
            Ok(request) => request,
            Err(error) => {
                let value = serde_json::from_slice::<serde_json::Value>(&self.line).ok();
                let id = value.as_ref().and_then(|value| value.get("id")?.as_u64());
                if let Some(id) = id
                    && value
                        .as_ref()
                        .is_some_and(|value| value.get("write").is_some())
                {
                    self.files.abort_write(id);
                }
                self.error(id, error, answers);
                return;
            }
        };
        let id = request.id;
        match &request.action {
            SandboxRequestAction::Signal(signal) if !(1..=64).contains(signal) => {
                answers.error(Some(id), "signal must be between 1 and 64");
                return;
            }
            SandboxRequestAction::Stdin(_) | SandboxRequestAction::Signal(_) => {
                if self.last_id.is_none_or(|last_id| id > last_id) {
                    answers.error(Some(id), "id has never run");
                    return;
                }
            }
            SandboxRequestAction::Write(_) if self.files.is_writing(id) => {}
            _ => {
                if let Some(last_id) = self.last_id
                    && id <= last_id
                {
                    answers.error(Some(id), format!("id must be greater than {last_id}"));
                    return;
                }
                self.last_id = Some(id);
            }
        }
        let result = match request.action {
            SandboxRequestAction::Run(run) => commands.run(id, run, poller, answers),
            SandboxRequestAction::Stdin(input) => commands.stdin(id, input, poller),
            SandboxRequestAction::Signal(signal) => commands.signal(id, signal),
            SandboxRequestAction::Write(piece) => self
                .files
                .write(id, piece)
                .map(|last| {
                    if last {
                        answers.push(Some(id), SandboxAnswerKind::Written(true));
                    }
                })
                .map_err(|error| error.to_string()),
            SandboxRequestAction::Read(read) => self
                .files
                .read(id, &read.path)
                .map_err(|error| error.to_string()),
        };
        if let Err(error) = result {
            answers.error(Some(id), error);
        }
    }

    fn error(&mut self, id: Option<u64>, error: impl std::fmt::Display, answers: &mut Answers) {
        answers.error(id, error);
        if id.is_none() {
            self.files.abort_writes(answers);
        }
    }
}

impl Poller {
    pub fn watch(&self, fd: RawFd, events: i32) -> io::Result<bool> {
        let mut event = libc::epoll_event {
            events: events as u32,
            u64: fd as u64,
        };
        if unsafe { libc::epoll_ctl(self.0.as_raw_fd(), libc::EPOLL_CTL_MOD, fd, &mut event) } == 0
        {
            return Ok(true);
        }
        if io::Error::last_os_error().raw_os_error() == Some(libc::EPERM) {
            return Ok(false);
        }
        if io::Error::last_os_error().raw_os_error() != Some(libc::ENOENT) {
            return Err(io::Error::last_os_error());
        }
        if unsafe { libc::epoll_ctl(self.0.as_raw_fd(), libc::EPOLL_CTL_ADD, fd, &mut event) } == 0
        {
            return Ok(true);
        }
        let error = io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::EPERM) {
            Ok(false)
        } else {
            Err(error)
        }
    }

    pub fn forget(&self, fd: RawFd) -> io::Result<()> {
        if unsafe {
            libc::epoll_ctl(
                self.0.as_raw_fd(),
                libc::EPOLL_CTL_DEL,
                fd,
                std::ptr::null_mut(),
            )
        } < 0
            && io::Error::last_os_error().raw_os_error() != Some(libc::ENOENT)
        {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

impl Answers {
    pub fn push(&mut self, id: Option<u64>, answer: SandboxAnswerKind) {
        serde_json::to_writer(&mut self.buffer, &SandboxAnswer { id, answer })
            .expect("JSON answer");
        self.buffer.push(b'\n');
    }

    pub fn error(&mut self, id: Option<u64>, error: impl std::fmt::Display) {
        self.push(id, SandboxAnswerKind::Error(error.to_string()));
    }

    fn flush(&mut self) -> io::Result<bool> {
        let mut remaining = self.buffer.as_slice();
        while !remaining.is_empty() {
            let count = unsafe { libc::write(1, remaining.as_ptr().cast(), remaining.len()) };
            if count < 0 {
                return match io::Error::last_os_error().kind() {
                    io::ErrorKind::Interrupted => continue,
                    io::ErrorKind::BrokenPipe => Ok(false),
                    _ => Err(io::Error::last_os_error()),
                };
            }
            if count == 0 {
                return Err(io::ErrorKind::WriteZero.into());
            }
            remaining = &remaining[count as usize..];
        }
        self.buffer.clear();
        Ok(true)
    }
}

pub fn descriptor(fd: RawFd) -> io::Result<OwnedFd> {
    if fd < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(unsafe { OwnedFd::from_raw_fd(fd) })
    }
}

pub fn nonblocking(fd: RawFd) -> io::Result<()> {
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

extern "C" fn termination_handler(_: i32) {}

fn signal_fd() -> io::Result<OwnedFd> {
    unsafe {
        let mut action: libc::sigaction = std::mem::zeroed();
        libc::sigemptyset(&mut action.sa_mask);
        action.sa_sigaction = termination_handler as *const () as usize;
        for signal in [libc::SIGTERM, libc::SIGINT] {
            // PID 1 needs a handler for the host to forward these signals.
            if libc::sigaction(signal, &action, std::ptr::null_mut()) < 0 {
                return Err(io::Error::last_os_error());
            }
        }
        action.sa_sigaction = libc::SIG_DFL;
        libc::sigaction(libc::SIGCHLD, &action, std::ptr::null_mut());
        let mut mask = std::mem::zeroed();
        libc::sigemptyset(&mut mask);
        for signal in [libc::SIGCHLD, libc::SIGTERM, libc::SIGINT, libc::SIGPIPE] {
            libc::sigaddset(&mut mask, signal);
        }
        if libc::sigprocmask(libc::SIG_BLOCK, &mask, std::ptr::null_mut()) < 0 {
            return Err(io::Error::last_os_error());
        }
        descriptor(libc::signalfd(
            -1,
            &mask,
            libc::SFD_CLOEXEC | libc::SFD_NONBLOCK,
        ))
    }
}

fn take_signals(fd: RawFd) -> io::Result<bool> {
    loop {
        let mut signal = std::mem::MaybeUninit::<libc::signalfd_siginfo>::uninit();
        let count = unsafe {
            libc::read(
                fd,
                signal.as_mut_ptr().cast(),
                size_of::<libc::signalfd_siginfo>(),
            )
        };
        if count < 0 {
            return match io::Error::last_os_error().kind() {
                io::ErrorKind::WouldBlock => Ok(false),
                io::ErrorKind::Interrupted => continue,
                _ => Err(io::Error::last_os_error()),
            };
        }
        if count as usize != size_of::<libc::signalfd_siginfo>() {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "short signalfd read",
            ));
        }
        if matches!(
            unsafe { signal.assume_init() }.ssi_signo as i32,
            libc::SIGTERM | libc::SIGINT
        ) {
            return Ok(true);
        }
    }
}
