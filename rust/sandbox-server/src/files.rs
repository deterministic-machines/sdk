use std::collections::BTreeMap;
use std::fs::{File, FileType, OpenOptions};
use std::io::{self, Read, Write};
use std::os::fd::IntoRawFd;
use std::os::unix::fs::OpenOptionsExt;

use dvm_api::{ByteChunk, SandboxAnswerKind, SandboxWrite};

use crate::request_loop::Answers;

pub struct Files {
    writes: BTreeMap<u64, FileWrite>,
    reads: BTreeMap<u64, File>,
}

struct FileWrite {
    path: String,
    file: File,
}

impl Files {
    pub fn new() -> Self {
        Self {
            writes: BTreeMap::new(),
            reads: BTreeMap::new(),
        }
    }

    pub fn is_writing(&self, id: u64) -> bool {
        self.writes.contains_key(&id)
    }

    pub fn abort_write(&mut self, id: u64) {
        self.writes.remove(&id);
    }

    pub fn abort_writes(&mut self, answers: &mut Answers) {
        for (id, _) in std::mem::take(&mut self.writes) {
            answers.error(Some(id), "write aborted after an error without an id");
        }
    }

    pub fn write(&mut self, id: u64, piece: SandboxWrite) -> io::Result<bool> {
        let mut write = match self.writes.remove(&id) {
            Some(write) if write.path == piece.path => write,
            Some(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "write path changed",
                ));
            }
            None => FileWrite {
                file: OpenOptions::new()
                    .write(true)
                    .create(true)
                    .truncate(true)
                    .mode(piece.mode.unwrap_or(0o644))
                    // A FIFO path must not block the request loop while opening.
                    .custom_flags(libc::O_NONBLOCK)
                    .open(&piece.path)?,
                path: piece.path,
            },
        };
        write.file.write_all(piece.data.bytes())?;
        if !piece.last {
            self.writes.insert(id, write);
        } else if unsafe { libc::close(write.file.into_raw_fd()) } < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(piece.last)
    }

    pub fn read(&mut self, id: u64, path: &str) -> io::Result<()> {
        require_regular_file(std::fs::metadata(path)?.file_type())?;
        // The path may become a FIFO between stat and open.
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(path)?;
        require_regular_file(file.metadata()?.file_type())?;
        self.reads.insert(id, file);
        Ok(())
    }

    pub fn advance_reads(&mut self, answers: &mut Answers) -> bool {
        let Some((&id, file)) = self.reads.first_key_value() else {
            return false;
        };
        let mut file = file;
        let mut bytes = [0; 8192];
        let answer = match file.read(&mut bytes) {
            Ok(0) => SandboxAnswerKind::End(true),
            Ok(count) => SandboxAnswerKind::Data(ByteChunk::new(bytes[..count].to_vec())),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => return true,
            Err(error) => SandboxAnswerKind::Error(error.to_string()),
        };
        if !matches!(answer, SandboxAnswerKind::Data(_)) {
            self.reads.remove(&id);
        }
        answers.push(Some(id), answer);
        true
    }
}

fn require_regular_file(kind: FileType) -> io::Result<()> {
    if kind.is_dir() {
        Err(io::Error::from_raw_os_error(libc::EISDIR))
    } else if !kind.is_file() {
        Err(io::Error::from_raw_os_error(libc::EINVAL))
    } else {
        Ok(())
    }
}
