//! The processes a scenario started, found and ended wherever they moved:
//! on Unix by listing every process (`ps`) with its parent, session and
//! start time, so a later process that reuses an id is never mistaken for
//! one of them; on Windows by a job object that ends every process in it.

#[cfg(unix)]
use std::process::Command;

/// A process: its id and when it started.
#[cfg(unix)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Started {
    pub(crate) pid: u32,
    since: String,
}

/// A running process: who it is and its parent.
#[cfg(unix)]
struct Listed {
    started: Started,
    parent: u32,
}

/// Every running process, or only `only`, as `ps` lists them. A process
/// that ended and waits only to be reaped is not running.
#[cfg(unix)]
fn processes(only: Option<u32>) -> Vec<Listed> {
    let mut command = Command::new("ps");
    match only {
        Some(pid) => command.args(["-p", &pid.to_string()]),
        None => command.arg("-A"),
    };
    let Ok(listing) = command
        .args(["-o", "pid=", "-o", "ppid=", "-o", "stat=", "-o", "lstart="])
        .env("LC_ALL", "C")
        .output()
    else {
        return Vec::new();
    };
    String::from_utf8_lossy(&listing.stdout)
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let pid = fields.next()?.parse().ok()?;
            let parent = fields.next()?.parse().ok()?;
            if fields.next()?.starts_with('Z') {
                return None;
            }
            let since = fields.collect::<Vec<_>>().join(" ");
            Some(Listed {
                started: Started { pid, since },
                parent,
            })
        })
        .collect()
}

/// The processes `roots` started, and theirs, now.
#[cfg(unix)]
pub(crate) fn descendants(roots: &[u32]) -> Vec<Started> {
    tree(&processes(None), roots.to_vec())
}

/// `roots` that run and every process under them in `all`.
#[cfg(unix)]
fn tree(all: &[Listed], roots: Vec<u32>) -> Vec<Started> {
    let mut found: Vec<Started> = Vec::new();
    let mut parents = roots;
    while let Some(parent) = parents.pop() {
        for child in all.iter().filter(|process| process.parent == parent) {
            if !found.iter().any(|known| known.pid == child.started.pid) {
                found.push(child.started.clone());
                parents.push(child.started.pid);
            }
        }
    }
    found
}

/// Every process in the sessions led by `leaders`, whatever group it moved
/// to, the leaders themselves, and every process under any of them: what a
/// shell started, in the background, under `nohup` or in a session of its
/// own while its parent still runs. On Linux, when this process adopts the
/// orphans of what it started ([`adopt_orphans`]), its own children count
/// too, as that is where a process goes whose parent ended: only the
/// agent's binary adopts them, and while a shell is shared it starts
/// nothing else.
#[cfg(unix)]
pub(crate) fn sessions(leaders: &[u32]) -> Vec<Started> {
    let all = processes(None);
    let mut roots: Vec<u32> = all
        .iter()
        .map(|process| process.started.pid)
        .filter(|pid| leaders.contains(pid) || leaders.contains(&session_of(*pid)))
        .collect();
    if adopts_orphans() {
        let own = std::process::id();
        roots.extend(
            all.iter()
                .filter(|process| process.parent == own)
                .map(|process| process.started.pid),
        );
    }
    let mut found: Vec<Started> = all
        .iter()
        .filter(|process| roots.contains(&process.started.pid))
        .map(|process| process.started.clone())
        .collect();
    for process in tree(&all, roots) {
        if !found.contains(&process) {
            found.push(process);
        }
    }
    found
}

/// The session `pid` is in; 0 when it cannot be read.
#[cfg(unix)]
fn session_of(pid: u32) -> u32 {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return 0;
    };
    // SAFETY: getsid only reads the session id of the process named.
    u32::try_from(unsafe { libc::getsid(pid) }).unwrap_or(0)
}

/// Makes this process adopt the orphans of every process it starts, so a
/// process that leaves its parent stays findable under it (Linux only;
/// elsewhere it does nothing). Call it once, before starting any.
pub fn adopt_orphans() {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    // SAFETY: PR_SET_CHILD_SUBREAPER takes one integer and changes only
    // whom this process's descendants are reparented to.
    unsafe {
        libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0);
    }
}

/// Whether this process adopts its descendants' orphans.
#[cfg(unix)]
fn adopts_orphans() -> bool {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        let mut adopts: libc::c_int = 0;
        // SAFETY: PR_GET_CHILD_SUBREAPER writes one integer to the pointer.
        let read = unsafe { libc::prctl(libc::PR_GET_CHILD_SUBREAPER, &mut adopts, 0, 0, 0) };
        read == 0 && adopts != 0
    }
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    false
}

/// Reaps `processes` that are this process's own children and ended, so an
/// adopted orphan does not linger as a zombie.
#[cfg(unix)]
pub(crate) fn reap(processes: &[Started]) {
    for process in processes {
        if let Ok(pid) = libc::pid_t::try_from(process.pid) {
            let mut status = 0;
            // SAFETY: WNOHANG never blocks; a pid that is not our child
            // only fails with ECHILD.
            unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG) };
        }
    }
}

/// Whether `started` still runs as the same process it was.
#[cfg(unix)]
pub(crate) fn same(started: &Started) -> bool {
    processes(Some(started.pid))
        .first()
        .is_some_and(|now| now.started == *started)
}

/// Sends `signal` to each of `processes` that is still the same process.
#[cfg(unix)]
pub(crate) fn signal(processes: &[Started], signal: libc::c_int) {
    for process in processes.iter().filter(|process| same(process)) {
        if let Ok(pid) = libc::pid_t::try_from(process.pid) {
            // SAFETY: signals a process checked to be the same one just before.
            unsafe { libc::kill(pid, signal) };
        }
    }
}

/// Whether the process `pid` runs: neither ended nor waiting to be reaped.
pub fn runs(pid: u32) -> bool {
    #[cfg(unix)]
    {
        !processes(Some(pid)).is_empty()
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::{CloseHandle, STILL_ACTIVE};
        use windows_sys::Win32::System::Threading::{
            GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
        };
        // SAFETY: opens the process for querying only; the handle is
        // closed below.
        let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if handle.is_null() {
            return false;
        }
        let mut code = 0u32;
        // SAFETY: `handle` is open and `code` is valid for the call.
        let read = unsafe { GetExitCodeProcess(handle, &mut code) };
        // SAFETY: `handle` was opened above and is closed once.
        unsafe { CloseHandle(handle) };
        read != 0 && code == STILL_ACTIVE as u32
    }
}

/// A job object that ends every process in it when it is ended or its
/// handle closes. Processes in it cannot leave it: it does not allow
/// breaking away, so what they start joins it too.
#[cfg(windows)]
pub(crate) struct Job(std::os::windows::io::OwnedHandle);

#[cfg(windows)]
impl Job {
    pub(crate) fn new() -> std::io::Result<Self> {
        use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
        use windows_sys::Win32::System::JobObjects::{
            CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject,
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        };

        // SAFETY: no attributes and no name: an anonymous job owned by the agent.
        let raw = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if raw.is_null() {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: `raw` is a new handle nothing else owns.
        let job = unsafe { OwnedHandle::from_raw_handle(raw) };
        // SAFETY: an all-zero limit is a valid value of that plain C struct.
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        // SAFETY: `limits` is valid for the call and its size is the one passed.
        let set = unsafe {
            SetInformationJobObject(
                job.as_raw_handle(),
                JobObjectExtendedLimitInformation,
                std::ptr::from_ref(&limits).cast(),
                u32::try_from(std::mem::size_of_val(&limits)).unwrap_or(u32::MAX),
            )
        };
        if set == 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(Self(job))
    }

    /// Puts the process `process` (a handle to it) in the job.
    pub(crate) fn assign(&self, process: std::os::windows::io::RawHandle) -> std::io::Result<()> {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::System::JobObjects::AssignProcessToJobObject;
        // SAFETY: both handles are open for the call's duration.
        if unsafe { AssignProcessToJobObject(self.0.as_raw_handle(), process) } == 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    }

    /// Puts the process with id `pid` in the job.
    pub(crate) fn assign_pid(&self, pid: u32) -> std::io::Result<()> {
        use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
        use windows_sys::Win32::System::Threading::{
            OpenProcess, PROCESS_SET_QUOTA, PROCESS_TERMINATE,
        };
        // SAFETY: opens the process with the rights a job assignment needs.
        let raw = unsafe { OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, 0, pid) };
        if raw.is_null() {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: `raw` is a new handle nothing else owns.
        let process = unsafe { OwnedHandle::from_raw_handle(raw) };
        self.assign(process.as_raw_handle())
    }

    /// Ends every process in the job; how many still run after `within`.
    pub(crate) fn end(&self, within: std::time::Duration) -> u32 {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::System::JobObjects::{
            JobObjectBasicAccountingInformation, QueryInformationJobObject, TerminateJobObject,
            JOBOBJECT_BASIC_ACCOUNTING_INFORMATION,
        };
        // SAFETY: the job's handle is open; its processes end with code 1.
        unsafe { TerminateJobObject(self.0.as_raw_handle(), 1) };
        let deadline = std::time::Instant::now() + within;
        loop {
            // SAFETY: an all-zero value is valid for that plain C struct.
            let mut accounting: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION =
                unsafe { std::mem::zeroed() };
            // SAFETY: `accounting` is valid for the call and its size is the one passed.
            let read = unsafe {
                QueryInformationJobObject(
                    self.0.as_raw_handle(),
                    JobObjectBasicAccountingInformation,
                    std::ptr::from_mut(&mut accounting).cast(),
                    u32::try_from(std::mem::size_of_val(&accounting)).unwrap_or(u32::MAX),
                    std::ptr::null_mut(),
                )
            };
            if read == 0 || accounting.ActiveProcesses == 0 || std::time::Instant::now() > deadline
            {
                return if read == 0 {
                    0
                } else {
                    accounting.ActiveProcesses
                };
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }

    pub(crate) fn into_handle(self) -> std::os::windows::io::OwnedHandle {
        self.0
    }
}
