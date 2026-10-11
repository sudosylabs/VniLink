//! Host-selected ConPTY. Pipe I/O and teardown have independent workers so
//! pending writes can be cancelled and output remains drained during Close.
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
        Arc,
    },
    thread::JoinHandle,
    time::Duration,
};
use vnilink::terminal::{
    HostTerminalFactory, TerminalBackend, TerminalCommand, TerminalEvent, TerminalSize,
    TERMINAL_CHUNK, TERMINAL_QUEUE,
};
use vnilink::{BackendStartup, Error};
use windows::{
    core::{PCWSTR, PWSTR},
    Win32::{
        Foundation::{HANDLE, WAIT_OBJECT_0},
        System::{
            Console::{ClosePseudoConsole, CreatePseudoConsole, ResizePseudoConsole, COORD, HPCON},
            Pipes::CreatePipe,
            Threading::{
                CreateProcessW, GetCurrentThreadId, GetExitCodeProcess,
                InitializeProcThreadAttributeList, OpenThread, TerminateProcess,
                UpdateProcThreadAttribute, WaitForSingleObject, EXTENDED_STARTUPINFO_PRESENT,
                LPPROC_THREAD_ATTRIBUTE_LIST, PROCESS_INFORMATION,
                PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE, STARTUPINFOEXW, THREAD_TERMINATE,
            },
            IO::CancelSynchronousIo,
        },
    },
};

pub struct WindowsTerminalFactory {
    program: PathBuf,
    directory: PathBuf,
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
        Ok(Self { program, directory })
    }
    pub fn for_current_user() -> Result<Self, Error> {
        let directory = std::env::var_os("USERPROFILE")
            .map(PathBuf::from)
            .ok_or(Error::Unsupported("host home directory"))?;
        let program = std::env::var_os("ComSpec")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("SystemRoot")
                    .map(|p| PathBuf::from(p).join("System32").join("cmd.exe"))
            })
            .ok_or(Error::Unsupported("host shell"))?;
        Self::new(program, directory)
    }
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

struct Attributes {
    _memory: Vec<u128>,
    list: LPPROC_THREAD_ATTRIBUTE_LIST,
}
impl Attributes {
    fn new(console: HPCON) -> Result<Self, Error> {
        let mut bytes = 0;
        unsafe {
            let _ = InitializeProcThreadAttributeList(None, 1, None, &mut bytes);
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
        while !self.join.is_finished() {
            if let Some(thread) = &self.thread {
                unsafe {
                    let _ = CancelSynchronousIo(handle(thread));
                }
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        let _ = self.join.join();
    }
}
struct Session {
    console: Option<HPCON>,
    process: Option<OwnedHandle>,
    reader: Option<JoinHandle<()>>,
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
                    let _ = TerminateProcess(handle(process), 1);
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
            let _ = reader.join();
        }
        self.process.take();
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        self.close(true);
    }
}
fn send(
    output: &tokio::sync::mpsc::Sender<TerminalEvent>,
    mut event: TerminalEvent,
    stop: &AtomicBool,
    drain: &AtomicBool,
) -> bool {
    loop {
        if stop.load(Ordering::Acquire) || drain.load(Ordering::Acquire) {
            return false;
        }
        match output.try_send(event) {
            Ok(()) => return true,
            Err(tokio::sync::mpsc::error::TrySendError::Full(value)) => event = value,
            Err(_) => return false,
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}
impl HostTerminalFactory for WindowsTerminalFactory {
    fn open(&self, size: TerminalSize, startup: BackendStartup) -> Result<TerminalBackend, Error> {
        startup.check()?;
        size.validate()?;
        let (input_read, mut input_write) = pipe()?;
        let (mut output_read, output_write) = pipe()?;
        let (output, events) = tokio::sync::mpsc::channel(TERMINAL_QUEUE);
        let stop = Arc::new(AtomicBool::new(false));
        let drain = Arc::new(AtomicBool::new(false));
        let writer_stop = Arc::new(AtomicBool::new(false));
        let out = output.clone();
        let ended = stop.clone();
        let discarding = drain.clone();
        let reader = std::thread::Builder::new()
            .name("vnilink-conpty-read".into())
            .spawn(move || {
                loop {
                    let mut bytes = vec![0; TERMINAL_CHUNK];
                    match output_read.read(&mut bytes) {
                        Ok(0) => break,
                        Ok(n) => {
                            bytes.truncate(n);
                            if !send(&out, TerminalEvent::Output(bytes), &ended, &discarding) {
                                // Closing the pipe during console teardown can deadlock
                                // older Windows. Discard only after explicit cancellation
                                // or receiver loss, and keep draining until native EOF.
                                discarding.store(true, Ordering::Release);
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
        startup.check()?;
        let attributes = Attributes::new(console)?;
        let mut info = STARTUPINFOEXW::default();
        info.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;
        info.lpAttributeList = attributes.list;
        let program = wide(self.program.as_os_str());
        let directory = wide(self.directory.as_os_str());
        let mut command: Vec<u16> = Some(34)
            .into_iter()
            .chain(self.program.as_os_str().encode_wide())
            .chain([34, 0])
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
        let writer = std::thread::Builder::new()
            .name("vnilink-conpty-write".into())
            .spawn(move || {
                let thread = unsafe { OpenThread(THREAD_TERMINATE, false, GetCurrentThreadId()) }
                    .map(|h| unsafe { OwnedHandle::from_raw_handle(h.0) });
                match thread {
                    Ok(thread) => {
                        if ready.send(Ok(thread)).is_err() {
                            return;
                        }
                    }
                    Err(error) => {
                        let _ = ready.send(Err(error));
                        return;
                    }
                }
                while !writer_quit.load(Ordering::Acquire) {
                    match incoming.recv_timeout(Duration::from_millis(20)) {
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
                        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
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
            while !session.stop.load(Ordering::Acquire) {
                let child = handle(session.process.as_ref().unwrap());
                if unsafe { WaitForSingleObject(child, 0) } == WAIT_OBJECT_0 {
                    let mut status = 0;
                    if unsafe { GetExitCodeProcess(child, &mut status) }.is_ok() {
                        code = Some(status);
                    }
                    break;
                }
                if session.writer.as_ref().unwrap().join.is_finished()
                    || session.reader.as_ref().unwrap().is_finished()
                {
                    break;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            let cancelled = session.stop.load(Ordering::Acquire);
            session.close(cancelled || code.is_none());
            if !cancelled {
                // Cleanup may discard after an I/O failure. Publish the terminal
                // status independently; ordinary child exit drains prior output.
                let event = code
                    .map(TerminalEvent::Exited)
                    .unwrap_or_else(|| TerminalEvent::Failed("Windows terminal I/O ended".into()));
                let clear = AtomicBool::new(false);
                let _ = send(&output, event, &session.stop, &clear);
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
    #[ignore = "requires actual Windows 10 1809+ ConPTY; GNU cross-compilation does not execute it"]
    fn conpty_executes_unicode_and_releases_its_process_on_close() {
        let factory = WindowsTerminalFactory::for_current_user().unwrap();
        let mut backend = factory
            .open(
                TerminalSize {
                    rows: 24,
                    columns: 80,
                },
                BackendStartup::new(Duration::from_secs(5)),
            )
            .unwrap();
        backend
            .commands
            .send(TerminalCommand::Input(
                "chcp 65001\r\necho VNI_CONPTY_RE^ADY_α\r\n"
                    .as_bytes()
                    .to_vec(),
            ))
            .unwrap();
        let process = unsafe {
            windows::Win32::System::Threading::OpenProcess(
                windows::Win32::System::Threading::PROCESS_SYNCHRONIZE,
                false,
                backend.process_id,
            )
        }
        .unwrap();
        let process = unsafe { OwnedHandle::from_raw_handle(process.0) };
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let mut bytes = Vec::new();
        loop {
            if let Ok(TerminalEvent::Output(chunk)) = backend.output.try_recv() {
                bytes.extend(chunk);
            }
            if bytes
                .windows("VNI_CONPTY_READY_α".len())
                .any(|b| b == "VNI_CONPTY_READY_α".as_bytes())
            {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "ConPTY output timed out"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        drop(backend);
        assert_eq!(
            unsafe { WaitForSingleObject(handle(&process), 5000) },
            WAIT_OBJECT_0
        );
    }
}
