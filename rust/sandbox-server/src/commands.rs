use std::collections::{BTreeMap, VecDeque};
use std::ffi::{CString, OsString};
use std::io;
use std::os::fd::{AsRawFd, OwnedFd, RawFd};
use std::os::unix::ffi::OsStrExt;

use dvm_api::{
    ByteChunk, SandboxAnswerKind, SandboxExited, SandboxProgram, SandboxRun, SandboxStarted,
    SandboxStdin,
};

use crate::command_user::CommandUser;
use crate::request_loop::{Answers, Poller, descriptor, nonblocking};

pub struct Commands {
    shell: Option<(&'static str, &'static str)>,
    commands: BTreeMap<u64, Command>,
}

struct Command {
    pid: i32,
    exited: bool,
    stdin: Option<OwnedFd>,
    pending: VecDeque<u8>,
    close_stdin: bool,
    stdout: Option<OwnedFd>,
    stderr: Option<OwnedFd>,
}

struct Execution {
    argv: Vec<CString>,
    environment: Vec<CString>,
    candidates: Vec<CString>,
    cwd: Option<CString>,
    /// An unresolvable user still starts the command, which then exits 126
    /// with the reason on stderr, as an unrunnable program does.
    user: Option<Result<CommandUser, String>>,
    last_fd: i32,
}

impl Commands {
    pub fn new() -> io::Result<Self> {
        // A VM's init already adopts orphans; host tests need the same behavior.
        if unsafe { libc::getpid() } != 1
            && unsafe { libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0) } < 0
        {
            return Err(io::Error::last_os_error());
        }
        let shell = if std::path::Path::new("/bin/bash").exists() {
            Some(("/bin/bash", "-lc"))
        } else if std::path::Path::new("/bin/sh").exists() {
            Some(("/bin/sh", "-c"))
        } else {
            None
        };
        Ok(Self {
            shell,
            commands: BTreeMap::new(),
        })
    }

    pub fn run(
        &mut self,
        id: u64,
        run: SandboxRun,
        poller: &Poller,
        answers: &mut Answers,
    ) -> Result<(), String> {
        let argv = match run.program {
            SandboxProgram::Argv(argv) if !argv.is_empty() && !argv[0].is_empty() => argv,
            SandboxProgram::Argv(_) => return Err("argv must contain a program".into()),
            SandboxProgram::Script(script) => {
                let (shell, flag) = self.shell.ok_or("no /bin/bash or /bin/sh for script")?;
                vec![shell.into(), flag.into(), script]
            }
        };
        let spawn = || -> io::Result<Command> {
            let (stdout, output_write) = pipe()?;
            let (stderr, error_write) = pipe()?;
            let (input_read, stdin) = if run.stdin {
                let (read, write) = pipe()?;
                (read, Some(write))
            } else {
                (
                    descriptor(unsafe {
                        libc::open(c"/dev/null".as_ptr(), libc::O_RDONLY | libc::O_CLOEXEC)
                    })?,
                    None,
                )
            };
            let user = run.user.as_deref().map(CommandUser::of_image);
            let execution = Execution::prepare(&argv, run.env, run.cwd, user)?;
            nonblocking(stdout.as_raw_fd())?;
            nonblocking(stderr.as_raw_fd())?;
            if let Some(stdin) = &stdin {
                nonblocking(stdin.as_raw_fd())?;
            }
            poller.watch(stdout.as_raw_fd(), libc::EPOLLIN)?;
            poller.watch(stderr.as_raw_fd(), libc::EPOLLIN)?;
            let pid = unsafe { libc::fork() };
            if pid < 0 {
                return Err(io::Error::last_os_error());
            }
            if pid == 0 {
                execution.exec(
                    input_read.as_raw_fd(),
                    output_write.as_raw_fd(),
                    error_write.as_raw_fd(),
                );
            }
            // Either side may run first; both establish the same group before signals.
            unsafe { libc::setpgid(pid, pid) };
            Ok(Command {
                pid,
                exited: false,
                stdin,
                pending: VecDeque::new(),
                close_stdin: false,
                stdout: Some(stdout),
                stderr: Some(stderr),
            })
        };
        let command = spawn().map_err(|error| error.to_string())?;
        answers.push(
            Some(id),
            SandboxAnswerKind::Started(SandboxStarted {
                pid: command.pid,
                argv,
            }),
        );
        self.commands.insert(id, command);
        Ok(())
    }

    pub fn stdin(&mut self, id: u64, input: SandboxStdin, poller: &Poller) -> Result<(), String> {
        let Some(command) = self.commands.get_mut(&id) else {
            return Ok(());
        };
        if command.exited || command.stdin.is_none() {
            return Ok(());
        }
        if command.close_stdin {
            return Err("stdin is closing".into());
        }
        command.pending.extend(input.data.bytes());
        command.close_stdin = input.close;
        command
            .flush_stdin(poller)
            .map_err(|error| error.to_string())
    }

    pub fn signal(&self, id: u64, signal: i32) -> Result<(), String> {
        let Some(command) = self.commands.get(&id) else {
            return Ok(());
        };
        if command.exited {
            return Ok(());
        }
        if unsafe { libc::kill(-command.pid, signal) } < 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() != Some(libc::ESRCH) {
                return Err(error.to_string());
            }
        }
        Ok(())
    }

    pub fn ready(&mut self, fd: RawFd, poller: &Poller, answers: &mut Answers) -> io::Result<()> {
        for (&id, command) in &mut self.commands {
            if command.stdin.as_ref().map(AsRawFd::as_raw_fd) == Some(fd) {
                return command.flush_stdin(poller);
            }
            for (output, is_stderr) in [(&mut command.stdout, false), (&mut command.stderr, true)] {
                if output.as_ref().map(AsRawFd::as_raw_fd) == Some(fd) {
                    read_output(output, id, is_stderr, 8192, answers)?;
                    return Ok(());
                }
            }
        }
        Ok(())
    }

    pub fn reap(&mut self, answers: &mut Answers) -> io::Result<()> {
        loop {
            let mut status = 0;
            let pid = unsafe { libc::waitpid(-1, &mut status, libc::WNOHANG) };
            if pid == 0 {
                break;
            }
            if pid < 0 {
                let error = io::Error::last_os_error();
                match error.raw_os_error() {
                    Some(libc::ECHILD) => break,
                    Some(libc::EINTR) => continue,
                    _ => return Err(error),
                }
            }
            if !libc::WIFEXITED(status) && !libc::WIFSIGNALED(status) {
                continue;
            }
            if let Some((&id, command)) = self
                .commands
                .iter_mut()
                .find(|(_, c)| c.pid == pid && !c.exited)
            {
                for (output, is_stderr) in
                    [(&mut command.stdout, false), (&mut command.stderr, true)]
                {
                    if let Some(fd) = output.as_ref() {
                        // A writing descendant must not delay exited forever.
                        let mut remaining = 0i32;
                        if unsafe { libc::ioctl(fd.as_raw_fd(), libc::FIONREAD, &mut remaining) }
                            < 0
                        {
                            return Err(io::Error::last_os_error());
                        }
                        while remaining > 0 {
                            let count =
                                read_output(output, id, is_stderr, remaining as usize, answers)?;
                            if count == 0 {
                                break;
                            }
                            remaining -= count as i32;
                        }
                    }
                }
                command.exited = true;
                command.stdin = None;
                command.pending.clear();
                let code = if libc::WIFEXITED(status) {
                    libc::WEXITSTATUS(status)
                } else {
                    128 + libc::WTERMSIG(status)
                };
                answers.push(Some(id), SandboxAnswerKind::Exited(SandboxExited { code }));
            }
        }
        self.commands.retain(|_, command| {
            !command.exited || command.stdout.is_some() || command.stderr.is_some()
        });
        Ok(())
    }

    pub fn shutdown(&self) {
        for command in self.commands.values() {
            unsafe { libc::kill(-command.pid, libc::SIGKILL) };
            if !command.exited {
                unsafe { libc::kill(command.pid, libc::SIGKILL) };
            }
        }
    }
}

impl Command {
    fn flush_stdin(&mut self, poller: &Poller) -> io::Result<()> {
        let Some(stdin) = &self.stdin else {
            return Ok(());
        };
        let fd = stdin.as_raw_fd();
        while !self.pending.is_empty() {
            let bytes = self.pending.as_slices().0;
            let count = unsafe { libc::write(fd, bytes.as_ptr().cast(), bytes.len()) };
            if count < 0 {
                let error = io::Error::last_os_error();
                match error.kind() {
                    io::ErrorKind::Interrupted => continue,
                    io::ErrorKind::WouldBlock => break,
                    io::ErrorKind::BrokenPipe => {
                        self.stdin = None;
                        self.pending.clear();
                        return Ok(());
                    }
                    _ => return Err(error),
                }
            }
            self.pending.drain(..count as usize);
        }
        if !self.pending.is_empty() {
            poller.watch(fd, libc::EPOLLOUT)?;
        } else if self.close_stdin {
            self.stdin = None;
        } else {
            poller.forget(fd)?;
        }
        Ok(())
    }
}

fn pipe() -> io::Result<(OwnedFd, OwnedFd)> {
    let mut fds = [-1; 2];
    if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok((descriptor(fds[0])?, descriptor(fds[1])?))
}

fn read_output(
    output: &mut Option<OwnedFd>,
    id: u64,
    stderr: bool,
    limit: usize,
    answers: &mut Answers,
) -> io::Result<usize> {
    let Some(fd) = output.as_ref() else {
        return Ok(0);
    };
    let mut bytes = [0; 8192];
    let count = loop {
        let count = unsafe {
            libc::read(
                fd.as_raw_fd(),
                bytes.as_mut_ptr().cast(),
                bytes.len().min(limit),
            )
        };
        if count >= 0 || io::Error::last_os_error().kind() != io::ErrorKind::Interrupted {
            break count;
        }
    };
    if count < 0 {
        let error = io::Error::last_os_error();
        return match error.kind() {
            io::ErrorKind::WouldBlock => Ok(0),
            _ => Err(error),
        };
    }
    if count == 0 {
        *output = None;
        return Ok(0);
    }
    let bytes = ByteChunk::new(bytes[..count as usize].to_vec());
    answers.push(
        Some(id),
        if stderr {
            SandboxAnswerKind::Stderr(bytes)
        } else {
            SandboxAnswerKind::Stdout(bytes)
        },
    );
    Ok(count as usize)
}

impl Execution {
    fn prepare(
        argv: &[String],
        additions: BTreeMap<String, String>,
        cwd: Option<String>,
        user: Option<Result<CommandUser, String>>,
    ) -> io::Result<Self> {
        let mut environment: BTreeMap<OsString, OsString> = std::env::vars_os().collect();
        if let Some(Ok(user)) = &user {
            environment.insert("HOME".into(), user.home().into());
            if let Some(name) = user.name() {
                environment.insert("USER".into(), name.into());
            }
        }
        for (name, value) in additions {
            if name.is_empty() || name.contains('=') {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "invalid env name",
                ));
            }
            environment.insert(name.into(), value.into());
        }
        let program = &argv[0];
        let candidates = if program.contains('/') {
            vec![CString::new(program.as_bytes())?]
        } else {
            let path = environment
                .get(std::ffi::OsStr::new("PATH"))
                .map(|path| path.as_bytes())
                .unwrap_or(b"/bin:/usr/bin");
            path.split(|&byte| byte == b':')
                .map(|directory| {
                    let mut candidate = directory.to_vec();
                    if !candidate.is_empty() {
                        candidate.push(b'/');
                    }
                    candidate.extend_from_slice(program.as_bytes());
                    CString::new(candidate)
                })
                .collect::<Result<Vec<_>, _>>()?
        };
        let environment = environment
            .into_iter()
            .map(|(name, value)| {
                let mut entry = name.as_bytes().to_vec();
                entry.push(b'=');
                entry.extend_from_slice(value.as_bytes());
                CString::new(entry)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let last_fd = match std::fs::read_dir("/proc/self/fd") {
            Ok(entries) => entries
                .filter_map(Result::ok)
                .filter_map(|entry| entry.file_name().to_str()?.parse::<i32>().ok())
                .max()
                .unwrap_or(2),
            Err(_) => {
                let mut limit = std::mem::MaybeUninit::<libc::rlimit>::uninit();
                if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, limit.as_mut_ptr()) } < 0 {
                    return Err(io::Error::last_os_error());
                }
                unsafe { limit.assume_init() }.rlim_max.min(i32::MAX as u64) as i32
            }
        };
        Ok(Self {
            argv: argv
                .iter()
                .map(|arg| CString::new(arg.as_bytes()))
                .collect::<Result<_, _>>()?,
            environment,
            candidates,
            cwd: cwd.map(CString::new).transpose()?,
            user,
            last_fd,
        })
    }

    fn exec(&self, stdin: RawFd, stdout: RawFd, stderr: RawFd) -> ! {
        for (source, target) in [(stderr, 2), (stdout, 1), (stdin, 0)] {
            if unsafe { libc::dup2(source, target) } < 0 {
                self.fail("dup2", io::Error::last_os_error(), 126);
            }
        }
        for fd in 3..=self.last_fd {
            unsafe { libc::close(fd) };
        }
        if unsafe { libc::setpgid(0, 0) } < 0 {
            self.fail("setpgid", io::Error::last_os_error(), 126);
        }
        unsafe {
            // libc reserves two signal numbers; the kernel must reset those too.
            let action = [0u64; 4];
            for signal in 1..=64 {
                if signal != libc::SIGKILL
                    && signal != libc::SIGSTOP
                    && libc::syscall(libc::SYS_rt_sigaction, signal, action.as_ptr(), 0, 8) < 0
                {
                    self.fail("sigaction", io::Error::last_os_error(), 126);
                }
            }
            let mask = 0u64;
            if libc::syscall(libc::SYS_rt_sigprocmask, libc::SIG_SETMASK, &mask, 0, 8) < 0 {
                self.fail("sigprocmask", io::Error::last_os_error(), 126);
            }
        }
        match &self.user {
            None => {}
            Some(Ok(user)) => {
                if let Err(error) = user.switch() {
                    self.fail("user", error, 126);
                }
            }
            Some(Err(reason)) => self.fail("user", reason, 126),
        }
        if let Some(cwd) = &self.cwd
            && unsafe { libc::chdir(cwd.as_ptr()) } < 0
        {
            self.fail("cwd", io::Error::last_os_error(), 126);
        }
        let argv = self
            .argv
            .iter()
            .map(|arg| arg.as_ptr())
            .chain([std::ptr::null()])
            .collect::<Vec<_>>();
        let environment = self
            .environment
            .iter()
            .map(|entry| entry.as_ptr())
            .chain([std::ptr::null()])
            .collect::<Vec<_>>();
        let mut denied = false;
        for candidate in &self.candidates {
            unsafe { libc::execve(candidate.as_ptr(), argv.as_ptr(), environment.as_ptr()) };
            let error = io::Error::last_os_error();
            match error.raw_os_error() {
                Some(libc::ENOENT | libc::ENOTDIR) => {}
                Some(libc::EACCES) => denied = true,
                _ => self.fail("exec", error, 126),
            }
        }
        let error = if denied { libc::EACCES } else { libc::ENOENT };
        self.fail(
            "exec",
            io::Error::from_raw_os_error(error),
            if denied { 126 } else { 127 },
        );
    }

    fn fail(&self, operation: &str, error: impl std::fmt::Display, code: i32) -> ! {
        let message = format!("{}: {operation}: {error}\n", self.argv[0].to_string_lossy());
        let mut bytes = message.as_bytes();
        while !bytes.is_empty() {
            let count = unsafe { libc::write(2, bytes.as_ptr().cast(), bytes.len()) };
            if count > 0 {
                bytes = &bytes[count as usize..];
            } else if io::Error::last_os_error().kind() != io::ErrorKind::Interrupted {
                break;
            }
        }
        unsafe { libc::_exit(code) }
    }
}
