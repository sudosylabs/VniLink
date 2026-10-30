// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! Host-selected ConPTY. Pipe I/O and teardown have independent workers so
//! pending writes can be cancelled and output remains drained during Close.
//! Each worker blocks on its pipe, queue or handles, so input and output move
//! as soon as ConPTY allows.
// trace: records every ignored failure
use crate::terminal_io::write_input;
use std::{
    fs::File,
    io::Read,
    os::windows::{
        ffi::OsStrExt,
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
    },
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{Receiver, RecvTimeoutError},
        Arc,
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};
use vnilink::terminal::{
    HostTerminalFactory, ShellChoice, TerminalBackend, TerminalCommand, TerminalEvent,
    TerminalSize, TERMINAL_CHUNK, TERMINAL_QUEUE,
};
use vnilink::{BackendStartup, Error};
use windows::{
    core::{PCWSTR, PWSTR},
    Win32::{
        Foundation::{HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT},
        System::{
            Console::{ClosePseudoConsole, CreatePseudoConsole, ResizePseudoConsole, COORD, HPCON},
            Pipes::CreatePipe,
            Threading::{
                CreateEventW, CreateProcessW, GetCurrentThreadId, GetExitCodeProcess,
                InitializeProcThreadAttributeList, OpenThread, SetEvent, TerminateProcess,
                UpdateProcThreadAttribute, WaitForMultipleObjects, WaitForSingleObject,
                EXTENDED_STARTUPINFO_PRESENT, LPPROC_THREAD_ATTRIBUTE_LIST, PROCESS_INFORMATION,
                PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE, STARTF_USESTDHANDLES, STARTUPINFOEXW,
                THREAD_SYNCHRONIZE, THREAD_TERMINATE,
            },
            IO::CancelSynchronousIo,
        },
    },
};

/// How often an idle writer or supervisor looks whether its terminal was closed.
const STOP_CHECK: Duration = Duration::from_millis(20);
/// How often closing asks a writer blocked in a write to give it up.
const CANCEL_RETRY: Duration = Duration::from_millis(5);
/// Output that follows other output this closely is a busy shell's.
const BUSY: Duration = Duration::from_millis(2);
/// How long a busy shell's output may gather into one chunk, as on Unix:
/// each chunk crosses the network on its own, while output after a pause,
/// such as a typed key's echo, goes at once.
const GATHER: Duration = Duration::from_millis(1);

pub struct WindowsTerminalFactory {
    program: PathBuf,
    directory: PathBuf,
    args: Vec<String>,
    id: Option<String>,
}
impl WindowsTerminalFactory {
    pub fn new(program: PathBuf, directory: PathBuf) -> Result<Self, Error> {
        if !program.is_absolute()
            || !program.is_file()
            || !directory.is_absolute()
            || !directory.is_dir()
            || program.as_os_str().encode_wide().any(|c| c == 0 || c == 34)
            || directory.as_os_str().encode_wide().any(|c| c == 0)
        {
            return Err(Error::InvalidSetting(
                "choose an absolute shell path and an existing working directory",
            ));
        }
        Ok(Self {
            program,
            directory,
            args: Vec::new(),
            id: None,
        })
    }
    /// The automatic shell, as VS Code's default: PowerShell when installed,
    /// then Windows PowerShell, then Command Prompt.
    pub fn for_current_user() -> Result<Self, Error> {
        Self::for_shell(&automatic_shell()?)
    }
    /// A shell the local user chose, in their profile directory.
    pub fn for_shell(shell: &ShellChoice) -> Result<Self, Error> {
        vnilink::terminal::validate_shell_args(&shell.args)?;
        let args = shell
            .args
            .iter()
            .map(|arg| arg.to_str().map(str::to_owned))
            .collect::<Option<Vec<_>>>()
            .ok_or(Error::InvalidSetting("shell arguments are out of range"))?;
        let directory = std::env::var_os("USERPROFILE")
            .map(PathBuf::from)
            .ok_or(Error::Unsupported("host home directory"))?;
        let mut factory = Self::new(shell.program.clone(), directory)?;
        if factory.program.to_str().is_none() {
            return Err(Error::InvalidSetting(
                "choose an absolute shell path and an existing working directory",
            ));
        }
        factory.args = args;
        factory.id = Some(shell.id.clone());
        Ok(factory)
    }
}

/// The shells this computer can offer, found as VS Code finds terminal profiles.
pub fn available_shells() -> Vec<ShellChoice> {
    crate::shells::detect(&crate::shells::SystemShellHost)
}

/// What "automatic" offers on this computer.
pub fn automatic_shell() -> Result<ShellChoice, Error> {
    automatic_of(&available_shells())
        .cloned()
        .ok_or(Error::Unsupported("host shell"))
}

/// What "automatic" offers among `shells`.
pub fn automatic_of(shells: &[ShellChoice]) -> Option<&ShellChoice> {
    crate::shells::automatic(shells)
}
fn native_error(_: windows::core::Error) -> Error {
    Error::Unsupported("Windows terminal native operation")
}
fn coord(size: TerminalSize) -> COORD {
    COORD {
        X: size.columns as i16,
        Y: size.rows as i16,
    }
}
fn handle(value: &impl AsRawHandle) -> HANDLE {
    HANDLE(value.as_raw_handle())
}
fn pipe() -> Result<(File, File), Error> {
    let mut read = HANDLE::default();
    let mut write = HANDLE::default();
    unsafe {
        CreatePipe(&mut read, &mut write, None, 0).map_err(native_error)?;
    }
    // Successful CreatePipe transfers one independently owned handle per end.
    Ok(unsafe {
        (
            File::from_raw_handle(read.0),
            File::from_raw_handle(write.0),
        )
    })
}
fn wide(value: &std::ffi::OsStr) -> Vec<u16> {
    value.encode_wide().chain(Some(0)).collect()
}
fn millis(value: Duration) -> u32 {
    u32::try_from(value.as_millis()).unwrap_or(u32::MAX)
}
/// A manual-reset event that wakes the supervisor once a worker ends.
fn event() -> Result<Arc<OwnedHandle>, Error> {
    let event = unsafe { CreateEventW(None, true, false, PCWSTR::null()) }.map_err(native_error)?;
    // A successful CreateEventW returns a handle this process owns.
    Ok(Arc::new(unsafe { OwnedHandle::from_raw_handle(event.0) }))
}
/// Signals its event when the worker holding it returns or unwinds.
struct Finished(Arc<OwnedHandle>);
impl Drop for Finished {
    fn drop(&mut self) {
        unsafe {
            ignored_hr!(
                vnilink::trace::TERMINAL,
                "a supervisor already gone needs no wake-up",
                SetEvent(handle(&*self.0))
            );
        }
    }
}

struct Attributes {
    _memory: Vec<u128>,
    list: LPPROC_THREAD_ATTRIBUTE_LIST,
}
impl Attributes {
    fn new(console: HPCON) -> Result<Self, Error> {
        let mut bytes = 0;
        unsafe {
            // Asks only for the size: it fails with ERROR_INSUFFICIENT_BUFFER by design.
            let _sized = InitializeProcThreadAttributeList(None, 1, None, &mut bytes);
        }
        if bytes == 0 || bytes > 1024 * 1024 {
            return Err(Error::Unsupported("Windows process attributes"));
        }
        let mut memory = vec![0u128; bytes.div_ceil(std::mem::size_of::<u128>())];
        let list = LPPROC_THREAD_ATTRIBUTE_LIST(memory.as_mut_ptr().cast());
        unsafe {
            InitializeProcThreadAttributeList(Some(list), 1, None, &mut bytes)
                .map_err(native_error)?;
        }
        let result = Self {
            _memory: memory,
            list,
        };
        unsafe {
            UpdateProcThreadAttribute(
                list,
                0,
                PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE as usize,
                Some(console.0 as *const std::ffi::c_void),
                std::mem::size_of::<HPCON>(),
                None,
                None,
            )
            .map_err(native_error)?;
        }
        Ok(result)
    }
}
impl Drop for Attributes {
    fn drop(&mut self) {
        unsafe {
            windows::Win32::System::Threading::DeleteProcThreadAttributeList(self.list);
        }
    }
}
struct IoWorker {
    join: JoinHandle<()>,
    thread: Option<OwnedHandle>,
}
impl IoWorker {
    fn stop(self) {
        // Cancellation races the next write. Repeat until the worker has
        // observed its stop flag and exited; no thread is forcibly terminated.
        if let Some(thread) = &self.thread {
            loop {
                unsafe {
                    // Nothing pending to cancel is its usual answer between retries.
                    let _cancelled = CancelSynchronousIo(handle(thread));
                }
                if unsafe { WaitForSingleObject(handle(thread), millis(CANCEL_RETRY)) }
                    != WAIT_TIMEOUT
                {
                    break;
                }
            }
        }
        if self.join.join().is_err() {
            tracing::warn!(target: vnilink::trace::TERMINAL, "the terminal writer panicked");
        }
    }
}
struct Session {
    console: Option<HPCON>,
    process: Option<OwnedHandle>,
    reader: Option<JoinHandle<()>>,
    gatherer: Option<JoinHandle<()>>,
    writer: Option<IoWorker>,
    stop: Arc<AtomicBool>,
    writer_stop: Arc<AtomicBool>,
    drain: Arc<AtomicBool>,
}
impl Session {
    fn close(&mut self, cancel: bool) {
        if cancel {
            self.drain.store(true, Ordering::Release);
        }
        self.writer_stop.store(true, Ordering::Release);
        if let Some(process) = &self.process {
            unsafe {
                if WaitForSingleObject(handle(process), 0) != WAIT_OBJECT_0 {
                    ignored_hr!(
                        vnilink::trace::TERMINAL,
                        "a shell that cannot be ended ends with its console",
                        TerminateProcess(handle(process), 1)
                    );
                }
            }
        }
        if let Some(writer) = self.writer.take() {
            writer.stop();
        }
        // Reader continues draining here, even on pre-24H2 Windows where Close
        // blocks until clients disconnect. It observes external cancellation
        // while waiting for bounded output admission.
        if let Some(console) = self.console.take() {
            unsafe {
                ClosePseudoConsole(console);
            }
        }
        if let Some(reader) = self.reader.take() {
            if reader.join().is_err() {
                tracing::warn!(target: vnilink::trace::TERMINAL, "the terminal reader panicked");
            }
        }
        if let Some(gatherer) = self.gatherer.take() {
            if gatherer.join().is_err() {
                tracing::warn!(target: vnilink::trace::TERMINAL, "the terminal output gatherer panicked");
            }
        }
        self.process.take();
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        self.close(true);
    }
}
/// Hands what the reader read on as output, gathering a busy shell's pieces
/// into chunks of up to `TERMINAL_CHUNK` for at most `GATHER`. A full output
/// queue holds it, and so the reader and the shell, until the consumer takes
/// a chunk or goes away. It returns once output is discarded, leaving the
/// reader to drain the pipe alone.
fn gather(
    pieces: Receiver<Vec<u8>>,
    output: tokio::sync::mpsc::Sender<TerminalEvent>,
    drain: Arc<AtomicBool>,
    _finished: Finished,
) {
    let mut carried = None;
    let mut last_output: Option<Instant> = None;
    loop {
        let mut chunk = match carried.take() {
            Some(chunk) => chunk,
            None => match pieces.recv() {
                Ok(piece) => piece,
                Err(_) => return,
            },
        };
        let busy = last_output.is_some_and(|at| at.elapsed() < BUSY);
        let gathered = Instant::now() + GATHER;
        while busy && chunk.len() < TERMINAL_CHUNK {
            let left = gathered.saturating_duration_since(Instant::now());
            match pieces.recv_timeout(left) {
                Ok(piece) => {
                    let room = TERMINAL_CHUNK - chunk.len();
                    if piece.len() > room {
                        chunk.extend_from_slice(&piece[..room]);
                        carried = Some(piece[room..].to_vec());
                    } else {
                        chunk.extend(piece);
                    }
                }
                // The reader's end is seen at the next piece.
                Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => break,
            }
        }
        if drain.load(Ordering::Acquire)
            || output.blocking_send(TerminalEvent::Output(chunk)).is_err()
        {
            return;
        }
        last_output = Some(Instant::now());
    }
}
impl HostTerminalFactory for WindowsTerminalFactory {
    fn shell_id(&self) -> Option<&str> {
        self.id.as_deref()
    }
    fn open(&self, size: TerminalSize, startup: BackendStartup) -> Result<TerminalBackend, Error> {
        startup.check()?;
        size.validate()?;
        let (input_read, mut input_write) = pipe()?;
        let (mut output_read, output_write) = pipe()?;
        let (output, events) = tokio::sync::mpsc::channel(TERMINAL_QUEUE);
        let stop = Arc::new(AtomicBool::new(false));
        let drain = Arc::new(AtomicBool::new(false));
        let writer_stop = Arc::new(AtomicBool::new(false));
        let finished = event()?;
        let (read, pieces) = std::sync::mpsc::sync_channel::<Vec<u8>>(TERMINAL_QUEUE);
        let reader = std::thread::Builder::new()
            .name("vnilink-conpty-read".into())
            .spawn(move || {
                let mut read = Some(read);
                let mut bytes = vec![0; TERMINAL_CHUNK];
                loop {
                    match output_read.read(&mut bytes) {
                        Ok(0) => break,
                        Ok(n) => {
                            // Closing the pipe during console teardown can deadlock
                            // older Windows. Once output is discarded, after explicit
                            // cancellation or receiver loss, keep draining until native EOF.
                            if read
                                .as_ref()
                                .is_some_and(|to| to.send(bytes[..n].to_vec()).is_err())
                            {
                                read = None;
                            }
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                        Err(_) => break,
                    }
                }
            })
            .map_err(|_| Error::Unsupported("Windows terminal reader"))?;
        let mut session = Session {
            console: None,
            process: None,
            reader: Some(reader),
            gatherer: None,
            writer: None,
            stop: stop.clone(),
            drain: drain.clone(),
            writer_stop: writer_stop.clone(),
        };
        let opened = unsafe {
            CreatePseudoConsole(coord(size), handle(&input_read), handle(&output_write), 0)
                .map_err(native_error)
        };
        drop(input_read);
        drop(output_write);
        let console = opened?;
        session.console = Some(console);
        let out = output.clone();
        let discarding = drain.clone();
        let done = Finished(finished.clone());
        session.gatherer = Some(
            std::thread::Builder::new()
                .name("vnilink-conpty-output".into())
                .spawn(move || gather(pieces, out, discarding, done))
                .map_err(|_| Error::Unsupported("Windows terminal output"))?,
        );
        startup.check()?;
        let attributes = Attributes::new(console)?;
        let mut info = STARTUPINFOEXW::default();
        info.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;
        // Explicit null standard handles make the child use the pseudoconsole.
        // Otherwise Windows can inherit the parent's redirected stdout/stderr
        // and bypass the terminal output pipe (Microsoft terminal issue #4585).
        info.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
        info.lpAttributeList = attributes.list;
        let program = wide(self.program.as_os_str());
        let directory = wide(self.directory.as_os_str());
        let args: Vec<&str> = self.args.iter().map(String::as_str).collect();
        let mut command: Vec<u16> =
            crate::shells::command_line(&self.program.to_string_lossy(), &args)
                .encode_utf16()
                .chain(Some(0))
                .collect();
        let mut process = PROCESS_INFORMATION::default();
        unsafe {
            CreateProcessW(
                PCWSTR(program.as_ptr()),
                Some(PWSTR(command.as_mut_ptr())),
                None,
                None,
                false,
                EXTENDED_STARTUPINFO_PRESENT,
                None,
                PCWSTR(directory.as_ptr()),
                &info.StartupInfo,
                &mut process,
            )
            .map_err(native_error)?;
        }
        session.process = Some(unsafe { OwnedHandle::from_raw_handle(process.hProcess.0) });
        drop(unsafe { OwnedHandle::from_raw_handle(process.hThread.0) });
        startup.check()?;
        let (commands, incoming) = std::sync::mpsc::sync_channel(TERMINAL_QUEUE);
        let (ready, acknowledgement) = std::sync::mpsc::sync_channel(1);
        let writer_quit = writer_stop.clone();
        let done = Finished(finished.clone());
        let writer = std::thread::Builder::new()
            .name("vnilink-conpty-write".into())
            .spawn(move || {
                let _done = done;
                // Closing cancels this thread's writes and waits for it to end.
                let thread = unsafe {
                    OpenThread(
                        THREAD_TERMINATE | THREAD_SYNCHRONIZE,
                        false,
                        GetCurrentThreadId(),
                    )
                }
                .map(|h| unsafe { OwnedHandle::from_raw_handle(h.0) });
                match thread {
                    Ok(thread) => {
                        if ready.send(Ok(thread)).is_err() {
                            return;
                        }
                    }
                    Err(error) => {
                        vnilink::ignored!(
                            vnilink::trace::TERMINAL,
                            "the opener stopped waiting for the terminal",
                            ready.send(Err(error))
                        );
                        return;
                    }
                }
                while !writer_quit.load(Ordering::Acquire) {
                    match incoming.recv_timeout(STOP_CHECK) {
                        Ok(TerminalCommand::Input(bytes)) => {
                            if bytes.is_empty()
                                || bytes.len() > TERMINAL_CHUNK
                                || write_input(&mut input_write, &bytes, &writer_quit).is_err()
                            {
                                break;
                            }
                        }
                        Ok(TerminalCommand::Resize(size)) => {
                            if size.validate().is_err()
                                || unsafe { ResizePseudoConsole(console, coord(size)) }.is_err()
                            {
                                break;
                            }
                        }
                        Ok(TerminalCommand::Redraw(size)) => {
                            let temporary = TerminalSize {
                                rows: if size.rows == 1 { 2 } else { size.rows - 1 },
                                columns: size.columns,
                            };
                            if size.validate().is_err()
                                || unsafe { ResizePseudoConsole(console, coord(temporary)) }
                                    .is_err()
                                || unsafe { ResizePseudoConsole(console, coord(size)) }.is_err()
                            {
                                break;
                            }
                        }
                        Err(RecvTimeoutError::Timeout) => continue,
                        Err(_) => break,
                    }
                }
            })
            .map_err(|_| Error::Unsupported("Windows terminal writer"))?;
        // The thread publishes an owned cancellation handle before any write.
        session.writer = Some(IoWorker {
            join: writer,
            thread: None,
        });
        let thread = startup.receive(&acknowledgement)?.map_err(native_error)?;
        session.writer.as_mut().unwrap().thread = Some(thread);
        startup.check()?;
        let process_id = process.dwProcessId;
        let supervisor = move || {
            let mut code = None;
            let child = handle(session.process.as_ref().unwrap());
            // The child's exit, or the writer or output ending, wakes this wait.
            let waits = [child, handle(&*finished)];
            while !session.stop.load(Ordering::Acquire) {
                match unsafe { WaitForMultipleObjects(&waits, false, millis(STOP_CHECK)) } {
                    WAIT_OBJECT_0 => {
                        let mut status = 0;
                        if unsafe { GetExitCodeProcess(child, &mut status) }.is_ok() {
                            code = Some(status);
                        }
                        break;
                    }
                    WAIT_TIMEOUT => {}
                    _ => break,
                }
            }
            let cancelled = session.stop.load(Ordering::Acquire);
            session.close(cancelled || code.is_none());
            if !cancelled {
                // Cleanup may discard after an I/O failure. Publish the terminal
                // status independently; ordinary child exit drains prior output.
                // A full queue holds it until the consumer takes it or goes away.
                let event = code
                    .map(TerminalEvent::Exited)
                    .unwrap_or_else(|| TerminalEvent::Failed("Windows terminal I/O ended".into()));
                vnilink::ignored!(
                    vnilink::trace::TERMINAL,
                    "the session already left the terminal",
                    output.blocking_send(event)
                );
            }
        };
        std::thread::Builder::new()
            .name("vnilink-conpty".into())
            .spawn(supervisor)
            .map_err(|_| Error::Unsupported("Windows terminal supervisor"))?;
        Ok(TerminalBackend {
            process_id,
            shell: self
                .program
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
            commands,
            output: events,
            stop,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    /// Output until it contains `marker`, waiting at most `within`.
    fn output_until(backend: &mut TerminalBackend, marker: &str, within: Duration) -> Vec<u8> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        let mut bytes = Vec::new();
        // The timer belongs to the runtime, so it is made inside it.
        let found = runtime.block_on(async {
            tokio::time::timeout(within, async {
                while let Some(event) = backend.output.recv().await {
                    if let TerminalEvent::Output(chunk) = event {
                        bytes.extend(chunk);
                        if bytes.windows(marker.len()).any(|b| b == marker.as_bytes()) {
                            return true;
                        }
                    }
                }
                false
            })
            .await
        });
        assert!(
            matches!(found, Ok(true)),
            "{marker} did not arrive: {}",
            String::from_utf8_lossy(&bytes)
        );
        bytes
    }
    fn open_process(backend: &TerminalBackend) -> OwnedHandle {
        let process = unsafe {
            windows::Win32::System::Threading::OpenProcess(
                windows::Win32::System::Threading::PROCESS_SYNCHRONIZE,
                false,
                backend.process_id,
            )
        }
        .unwrap();
        unsafe { OwnedHandle::from_raw_handle(process.0) }
    }
    fn cmd() -> WindowsTerminalFactory {
        let cmd = available_shells()
            .into_iter()
            .find(|shell| shell.id == "cmd")
            .unwrap();
        WindowsTerminalFactory::for_shell(&cmd).unwrap()
    }
    const SIZE: TerminalSize = TerminalSize {
        rows: 24,
        columns: 80,
    };
    #[test]
    fn already_cancelled_conpty_startup_does_not_open_a_console() {
        let factory = WindowsTerminalFactory::for_current_user().unwrap();
        assert!(matches!(
            factory.open(
                TerminalSize {
                    rows: 24,
                    columns: 80
                },
                BackendStartup::new(Duration::ZERO)
            ),
            Err(Error::Timeout)
        ));
    }
    #[test]
    fn conpty_executes_unicode_and_releases_its_process_on_close() {
        // cmd syntax: `^` keeps the echoed command line from containing the marker.
        let mut backend = cmd()
            .open(SIZE, BackendStartup::new(Duration::from_secs(5)))
            .unwrap();
        backend
            .commands
            .send(TerminalCommand::Input(
                "chcp 65001\r\necho VNI_CONPTY_RE^ADY_α\r\n"
                    .as_bytes()
                    .to_vec(),
            ))
            .unwrap();
        let process = open_process(&backend);
        output_until(&mut backend, "VNI_CONPTY_READY_α", Duration::from_secs(5));
        drop(backend);
        assert_eq!(
            unsafe { WaitForSingleObject(handle(&process), 5000) },
            WAIT_OBJECT_0
        );
    }
    /// A shell flooding output nobody reads holds the full queue; closing
    /// still ends it promptly.
    #[test]
    fn close_ends_a_shell_whose_output_waits_on_a_full_queue() {
        let mut backend = cmd()
            .open(SIZE, BackendStartup::new(Duration::from_secs(5)))
            .unwrap();
        backend
            .commands
            .send(TerminalCommand::Input(
                "for /l %i in (1,1,1000000) do @echo VNI_FLOOD_%i\r\n"
                    .as_bytes()
                    .to_vec(),
            ))
            .unwrap();
        let process = open_process(&backend);
        output_until(&mut backend, "VNI_FLOOD_1", Duration::from_secs(5));
        // Nothing reads further: the queue fills behind the shell.
        assert_eq!(
            unsafe { WaitForSingleObject(handle(&process), 500) },
            WAIT_TIMEOUT
        );
        let closed = Instant::now();
        drop(backend);
        assert_eq!(
            unsafe { WaitForSingleObject(handle(&process), 5000) },
            WAIT_OBJECT_0
        );
        assert!(
            closed.elapsed() < Duration::from_secs(1),
            "{:?}",
            closed.elapsed()
        );
    }
    /// Automatic is PowerShell when installed, as VS Code's default; it gets the
    /// same ConPTY round trip in PowerShell syntax.
    #[test]
    fn automatic_shell_is_powershell_and_runs_under_conpty() {
        let automatic = automatic_shell().unwrap();
        assert!(
            ["pwsh", "windows-powershell"].contains(&automatic.id.as_str()),
            "automatic shell is {}",
            automatic.id
        );
        let mut backend = WindowsTerminalFactory::for_shell(&automatic)
            .unwrap()
            .open(SIZE, BackendStartup::new(Duration::from_secs(10)))
            .unwrap();
        backend
            .commands
            .send(TerminalCommand::Input(
                "Write-Output ('VNI_PWSH_' + 'READY_α')\r\n"
                    .as_bytes()
                    .to_vec(),
            ))
            .unwrap();
        output_until(&mut backend, "VNI_PWSH_READY_α", Duration::from_secs(15));
    }
}
