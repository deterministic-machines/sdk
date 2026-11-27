use std::collections::HashMap;
use std::sync::{Arc, Mutex, Weak};

use dvm_api::encode_frame;
use dvm_api::{
    FrameDecoder, MAX_HEADER, RunState, SandboxAnswer, SandboxAnswerKind, SandboxInput,
    SandboxInputState, SandboxRead, SandboxRequest, SandboxRequestAction, SandboxRun, SandboxStdin,
    SandboxWrite,
};
use tokio::sync::{Mutex as AsyncMutex, watch};
use tokio::task::AbortHandle;

use crate::command::{Command, CommandResult};
use crate::sandbox::SandboxCalls;
use crate::{Error, Result};

#[derive(Debug, Default)]
pub(crate) struct OperationState {
    started: bool,
    done: bool,
    error: Option<Error>,
    pub(crate) output: CommandResult,
    data: Vec<u8>,
}

#[derive(Debug)]
pub(crate) struct Operation {
    id: u64,
    pub(crate) state: watch::Receiver<OperationState>,
}
impl Operation {
    fn check(&self) -> Result<()> {
        self.state.borrow().error.clone().map_or(Ok(()), Err)
    }

    pub(crate) async fn wait(&self, started: bool) -> Result<()> {
        let mut state = self.state.clone();
        loop {
            {
                let value = state.borrow_and_update();
                if let Some(error) = &value.error {
                    return Err(error.clone());
                }
                if if started { value.started } else { value.done } {
                    return Ok(());
                }
            }
            state
                .changed()
                .await
                .map_err(|_| Error::Unexpected("session operation lost its answer".into()))?;
        }
    }
}

#[derive(Debug, Default)]
struct Operations {
    pending: HashMap<u64, watch::Sender<OperationState>>,
    failure: Option<Error>,
}

impl Operations {
    fn fail_pending(&mut self, error: Error) {
        for (_, operation) in self.pending.drain() {
            operation.send_modify(|state| state.error = Some(error.clone()));
        }
    }
    fn fail(&mut self, error: Error) {
        let error = self.failure.get_or_insert(error).clone();
        self.fail_pending(error);
    }
}

#[derive(Debug)]
struct InputWriter {
    offset: Option<u64>,
    ended: bool,
}

/// Offsets, frame encoding, and dispatch live only here. Cloned sandbox
/// handles share this session, while a separately connected handle does not.
#[derive(Debug)]
pub(crate) struct Session {
    calls: SandboxCalls,
    writer: AsyncMutex<InputWriter>,
    operations: Mutex<Operations>,
    reader: Mutex<Option<AbortHandle>>,
}

impl Session {
    const PIECE_SIZE: usize = 1024 * 1024;

    pub(crate) fn new(calls: SandboxCalls, offset: Option<u64>) -> Arc<Self> {
        Arc::new(Self {
            calls,
            writer: AsyncMutex::new(InputWriter {
                offset,
                ended: false,
            }),
            operations: Mutex::new(Operations::default()),
            reader: Mutex::new(None),
        })
    }

    pub(crate) async fn start(
        self: &Arc<Self>,
        run: SandboxRun,
        stdin: Option<Vec<u8>>,
    ) -> Result<Command> {
        let operation = self.begin(SandboxRequestAction::Run(run)).await?;
        operation.wait(true).await?;
        let command = Command {
            session: self.clone(),
            operation,
            stdin_closed: AsyncMutex::new(false),
        };
        if let Some(bytes) = stdin {
            if !bytes.is_empty() {
                command.write_stdin(&bytes).await?;
            }
            command.close_stdin().await?;
        }
        Ok(command)
    }

    pub(crate) async fn write_file(
        self: &Arc<Self>,
        path: String,
        bytes: &[u8],
        mode: Option<u32>,
    ) -> Result<()> {
        let operation = {
            let mut writer = self.writer.lock().await;
            self.ready(&mut writer, false).await?;
            let operation = self.register(writer.offset.unwrap())?;
            let result = async {
                let size = Self::PIECE_SIZE;
                let count = bytes.len().max(1).div_ceil(size);
                for index in 0..count {
                    operation.check()?;
                    let at = index * size;
                    let data = bytes
                        .get(at..(at + size).min(bytes.len()))
                        .unwrap_or_default();
                    let action = SandboxRequestAction::Write(SandboxWrite {
                        path: path.clone(),
                        mode,
                        last: index + 1 == count,
                    });
                    self.post(&mut writer, operation.id, action, data).await?;
                }
                Ok(())
            }
            .await;
            if let Err(error) = result {
                self.operations
                    .lock()
                    .unwrap()
                    .pending
                    .remove(&operation.id);
                return Err(error);
            }
            operation
        };
        operation.wait(false).await
    }

    pub(crate) async fn read_file(self: &Arc<Self>, path: String) -> Result<Vec<u8>> {
        let operation = self
            .begin(SandboxRequestAction::Read(SandboxRead { path }))
            .await?;
        operation.wait(false).await?;
        let bytes = operation.state.borrow().data.clone();
        Ok(bytes)
    }

    pub(crate) async fn signal(self: &Arc<Self>, operation: &Operation, number: i32) -> Result<()> {
        let mut writer = self.writer.lock().await;
        self.ready(&mut writer, false).await?;
        operation.check()?;
        self.post(
            &mut writer,
            operation.id,
            SandboxRequestAction::Signal(number),
            &[],
        )
        .await
    }

    pub(crate) async fn stdin(
        self: &Arc<Self>,
        operation: &Operation,
        bytes: &[u8],
        close: bool,
    ) -> Result<()> {
        let mut writer = self.writer.lock().await;
        self.ready(&mut writer, false).await?;
        operation.check()?;
        if close {
            return self
                .post(
                    &mut writer,
                    operation.id,
                    SandboxRequestAction::Stdin(SandboxStdin { close: true }),
                    &[],
                )
                .await;
        }
        let size = Self::PIECE_SIZE;
        for index in 0..bytes.len().max(1).div_ceil(size) {
            operation.check()?;
            let at = index * size;
            let data = bytes
                .get(at..(at + size).min(bytes.len()))
                .unwrap_or_default();
            self.post(
                &mut writer,
                operation.id,
                SandboxRequestAction::Stdin(SandboxStdin { close: false }),
                data,
            )
            .await?;
        }
        Ok(())
    }

    pub(crate) async fn end(self: &Arc<Self>) -> Result<i32> {
        {
            let mut writer = self.writer.lock().await;
            if !writer.ended {
                self.ready(&mut writer, true).await?;
                self.append(&mut writer, Vec::new(), true).await?;
                writer.ended = true;
            }
        }
        self.calls.wait().await
    }

    async fn begin(self: &Arc<Self>, action: SandboxRequestAction) -> Result<Operation> {
        let mut writer = self.writer.lock().await;
        self.ready(&mut writer, false).await?;
        let operation = self.register(writer.offset.unwrap())?;
        if let Err(error) = self.post(&mut writer, operation.id, action, &[]).await {
            self.operations
                .lock()
                .unwrap()
                .pending
                .remove(&operation.id);
            return Err(error);
        }
        Ok(operation)
    }

    fn register(&self, id: u64) -> Result<Operation> {
        let mut operations = self.operations.lock().unwrap();
        if let Some(error) = &operations.failure {
            return Err(error.clone());
        }
        let (send, state) = watch::channel(OperationState::default());
        operations.pending.insert(id, send);
        Ok(Operation { id, state })
    }

    async fn ready(self: &Arc<Self>, writer: &mut InputWriter, ending: bool) -> Result<()> {
        if !ending && let Some(error) = &self.operations.lock().unwrap().failure {
            return Err(error.clone());
        }
        if writer.ended {
            return Err(Error::SandboxRequest {
                reason: "sandbox input has ended".into(),
            });
        }
        if writer.offset.is_none() {
            self.append(writer, Vec::new(), false).await?;
        }
        let mut reader = self.reader.lock().unwrap();
        if reader.is_none() {
            *reader = Some(
                tokio::spawn(Self::read_answers(self.calls.clone(), Arc::downgrade(self)))
                    .abort_handle(),
            );
        }
        Ok(())
    }

    async fn append(&self, writer: &mut InputWriter, bytes: Vec<u8>, end: bool) -> Result<()> {
        let input = SandboxInput {
            offset: writer.offset.unwrap_or(0),
            end,
        };
        let state: SandboxInputState = self
            .calls
            .while_live(self.calls.client.input(
                &format!("{}/input", self.calls.path),
                &input,
                bytes,
            ))
            .await?;
        writer.offset = Some(state.length);
        Ok(())
    }

    async fn post(
        &self,
        writer: &mut InputWriter,
        id: u64,
        action: SandboxRequestAction,
        payload: &[u8],
    ) -> Result<()> {
        let bytes = encode_frame(&SandboxRequest { id, action }, payload);
        if bytes.len() - payload.len() - 1 > MAX_HEADER {
            return Err(Error::SandboxRequest {
                reason: "request header exceeds 65536 bytes".into(),
            });
        }
        for chunk in bytes.chunks(SandboxInput::MAX_CHUNK_BYTES) {
            self.append(writer, chunk.to_vec(), false).await?;
        }
        Ok(())
    }

    async fn read_answers(calls: SandboxCalls, session: Weak<Self>) {
        let result = async {
            let (mut offset, mut stderr_offset) = (0u64, 0u64);
            let mut decoder = FrameDecoder::default();
            loop {
                let output = calls
                    .client
                    .output_after(&format!("{}/output", calls.path), offset, stderr_offset)
                    .await?;
                offset += output.output.stdout.bytes().len() as u64;
                stderr_offset += output.output.stderr.bytes().len() as u64;
                decoder.push(output.output.stdout.bytes());
                {
                    let Some(session) = session.upgrade() else {
                        return Ok(());
                    };
                    while let Some(frame) = decoder.next_frame::<SandboxAnswer>() {
                        let frame = frame.map_err(|error| {
                            Error::Unexpected(format!("sandbox answer: {error}"))
                        })?;
                        session.dispatch(frame.header, &frame.payload);
                    }
                }
                if !output.more && !output.cut {
                    let code = calls.wait().await?;
                    return Err(Error::ended(calls.id, RunState::Exited { code }));
                }
            }
        }
        .await;
        if let Err(error) = result
            && let Some(session) = session.upgrade()
        {
            session.operations.lock().unwrap().fail(error);
        }
    }

    fn dispatch(&self, answer: SandboxAnswer, payload: &[u8]) {
        let mut operations = self.operations.lock().unwrap();
        if answer.id.is_none() {
            if let SandboxAnswerKind::Error(reason) = answer.answer {
                operations.fail_pending(Error::SandboxRequest { reason });
            }
            return;
        }
        let id = answer.id.unwrap();
        let Some(operation) = operations.pending.get(&id) else {
            return;
        };
        let mut finished = false;
        operation.send_modify(|state| {
            match answer.answer {
                SandboxAnswerKind::Started(_) => state.started = true,
                SandboxAnswerKind::Stdout(_) => state.output.stdout.extend_from_slice(payload),
                SandboxAnswerKind::Stderr(_) => state.output.stderr.extend_from_slice(payload),
                SandboxAnswerKind::Data(_) => state.data.extend_from_slice(payload),
                SandboxAnswerKind::Exited(exit) => {
                    state.output.exit_code = exit.code;
                    state.done = true;
                }
                SandboxAnswerKind::Written(_) | SandboxAnswerKind::End(_) => state.done = true,
                SandboxAnswerKind::Error(reason) => {
                    state.error = Some(Error::SandboxRequest { reason })
                }
            }
            finished = state.done || state.error.is_some();
        });
        if finished {
            operations.pending.remove(&id);
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        if let Some(reader) = self.reader.get_mut().unwrap() {
            reader.abort();
        }
    }
}
