//! Peer-bound persistent shell instances. No connection or UI owns the shell.
use super::*;
use crate::terminal::*;
use std::collections::VecDeque;
use std::sync::atomic::Ordering;
use std::time::Instant;

pub(super) struct HostedTerminal {
    pub(super) peer: [u8; 32],
    pub(super) info: Mutex<TerminalSessionInfo>,
    pub(super) commands: std::sync::mpsc::SyncSender<TerminalCommand>,
    pub(super) stop: Arc<AtomicBool>,
    /// Set when its viewer asked to close it, before `stop`, so its stream
    /// says the viewer closed it rather than the host.
    pub(super) closed_by_viewer: AtomicBool,
    pub(super) changed: tokio::sync::watch::Sender<u64>,
    policy: TerminalPolicy,
    log: Mutex<Log>,
    detached: Mutex<Option<Instant>>,
}

/// Stream attachment ownership must settle even when a parent JoinSet aborts
/// the transport future before its normal epilogue runs.
pub(super) struct Attachment {
    pub(super) terminal: Arc<HostedTerminal>,
    pub(super) explicit_close: Arc<AtomicBool>,
}
impl Drop for Attachment {
    fn drop(&mut self) {
        if self.explicit_close.load(Ordering::Acquire) {
            self.terminal.stop.store(true, Ordering::Release);
        }
        self.terminal.detach();
    }
}
struct Log {
    bytes: VecDeque<u8>,
    offset: u64,
    ended: Option<TerminalEvent>,
}
impl Log {
    fn push(&mut self, bytes: Vec<u8>, limit: usize) {
        self.bytes.extend(bytes);
        let excess = self.bytes.len().saturating_sub(limit);
        self.bytes.drain(..excess);
        self.offset = self.offset.saturating_add(excess as u64);
    }
}
impl HostedTerminal {
    pub(super) fn read(&self, offset: u64) -> (u64, Vec<u8>, Option<TerminalEvent>) {
        let log = lock(&self.log);
        let first = offset.max(log.offset);
        // A range starts where it asks, however much history is kept before it.
        let start = ((first - log.offset) as usize).min(log.bytes.len());
        let end = (start + TERMINAL_CHUNK).min(log.bytes.len());
        let bytes = log.bytes.range(start..end).copied().collect();
        (first, bytes, log.ended.clone())
    }
    pub(super) fn replay_start(&self) -> u64 {
        let log = lock(&self.log);
        log.offset.saturating_add(
            log.bytes
                .len()
                .saturating_sub(self.policy.replay_bytes as usize) as u64,
        )
    }
    fn detached_expired(&self, now: Instant) -> bool {
        lock(&self.detached)
            .and_then(|at| now.checked_duration_since(at))
            .is_some_and(|elapsed| {
                elapsed >= Duration::from_secs(u64::from(self.policy.detached_timeout_secs))
            })
    }
    pub(super) fn detach(&self) {
        lock(&self.info).attached = false;
        *lock(&self.detached) = Some(Instant::now());
        if !lock(&self.info).persistent {
            self.stop.store(true, Ordering::Release);
        }
    }
    pub(super) fn attach(&self, size: TerminalSize) -> Result<(), Error> {
        let mut info = lock(&self.info);
        if self.stop.load(Ordering::Acquire) {
            return Err(Error::NoSession);
        }
        if info.attached {
            return Err(Error::Busy);
        }
        // The redraw and restore are admitted as one command, so a full
        // queue cannot leave the terminal at a temporary size.
        self.commands
            .try_send(TerminalCommand::Redraw(size))
            .map_err(|_| Error::Busy)?;
        info.size = size;
        info.attached = true;
        *lock(&self.detached) = None;
        Ok(())
    }
}

pub(super) fn start(
    shared: Arc<Shared>,
    peer: [u8; 32],
    mut backend: TerminalBackend,
    options: &TerminalOptions,
    policy: TerminalPolicy,
) -> Result<Arc<HostedTerminal>, Error> {
    let id = rand::random::<[u8; 16]>()
        .iter()
        .map(|v| format!("{v:02x}"))
        .collect::<String>();
    let terminal = Arc::new(HostedTerminal {
        peer,
        policy,
        info: Mutex::new(TerminalSessionInfo {
            id: id.clone(),
            process_id: backend.process_id,
            shell: backend.shell.clone(),
            size: options.size,
            persistent: options.persistent,
            attached: true,
        }),
        commands: backend.commands.clone(),
        stop: backend.stop.clone(),
        closed_by_viewer: AtomicBool::new(false),
        changed: tokio::sync::watch::channel(0).0,
        log: Mutex::new(Log {
            bytes: VecDeque::new(),
            offset: 0,
            ended: None,
        }),
        detached: Mutex::new(None),
    });
    {
        let mut sessions = lock(&shared.terminal_sessions);
        if sessions.len() >= usize::from(policy.max_sessions) {
            return Err(Error::Busy);
        }
        sessions.insert(id.clone(), terminal.clone());
    }
    let runtime = terminal.clone();
    // Called on this core's runtime, after backend startup completed off-thread.
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_millis(25));
        loop {
            let event = tokio::select! {
                event = backend.output.recv(), if lock(&runtime.log).ended.is_none() => event,
                _ = ticker.tick() => {
                    let permitted = { let state = lock(&shared.state); !state.shut_down && state.trust.service_granted(&peer, crate::ServiceKind::Terminal) };
                    let expired = runtime.detached_expired(Instant::now());
                    if runtime.stop.load(Ordering::Acquire) || !permitted || expired { break; }
                    continue;
                }
            };
            let mut log = lock(&runtime.log);
            match event {
                Some(TerminalEvent::Output(bytes)) => {
                    if bytes.is_empty() || bytes.len() > TERMINAL_CHUNK {
                        log.ended = Some(TerminalEvent::Failed(
                            "terminal backend emitted an invalid chunk".into(),
                        ));
                    } else {
                        log.push(bytes, runtime.policy.history_bytes as usize);
                    }
                }
                Some(event) => log.ended = Some(event),
                None => log.ended = Some(TerminalEvent::Failed("terminal backend ended".into())),
            }
            drop(log);
            runtime
                .changed
                .send_modify(|value| *value = value.wrapping_add(1));
        }
        runtime.stop.store(true, Ordering::Release);
        runtime.changed.send_replace(u64::MAX);
        lock(&shared.terminal_sessions).remove(&id);
        // Dropping the backend cancels/reaps the shell even when no stream exists.
    });
    Ok(terminal)
}

#[cfg(test)]
mod attachment_tests {
    use super::*;
    #[test]
    fn aborting_attachment_detaches_and_retains_only_opted_in_shells() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            for (persistent, explicit) in [(true, false), (false, false), (true, true)] {
                let (commands, _rx) = std::sync::mpsc::sync_channel(1);
                let terminal = Arc::new(HostedTerminal {
                    peer: [0; 32],
                    closed_by_viewer: AtomicBool::new(false),
                    policy: TerminalPolicy::default(),
                    info: Mutex::new(TerminalSessionInfo {
                        id: "test".into(),
                        process_id: 0,
                        shell: "test".into(),
                        size: TerminalSize {
                            rows: 24,
                            columns: 80,
                        },
                        persistent,
                        attached: true,
                    }),
                    commands,
                    stop: Arc::new(AtomicBool::new(false)),
                    changed: tokio::sync::watch::channel(0).0,
                    log: Mutex::new(Log {
                        bytes: VecDeque::new(),
                        offset: 0,
                        ended: None,
                    }),
                    detached: Mutex::new(None),
                });
                let attachment = Attachment {
                    terminal: terminal.clone(),
                    explicit_close: Arc::new(AtomicBool::new(explicit)),
                };
                let (ready, entered) = tokio::sync::oneshot::channel();
                let task = tokio::spawn(async move {
                    let _owned = attachment;
                    ready.send(()).unwrap();
                    std::future::pending::<()>().await;
                });
                entered.await.unwrap();
                task.abort();
                assert!(task.await.unwrap_err().is_cancelled());
                assert!(!lock(&terminal.info).attached);
                assert!(lock(&terminal.detached).is_some());
                assert_eq!(
                    terminal.stop.load(Ordering::Acquire),
                    !persistent || explicit
                );
            }
        });
    }
}

/// Reserves capacity before allocating a native PTY. Dropping an aborted Open
/// releases its reservation, including while its blocking backend is unwinding.
pub(super) struct Reservation {
    shared: Arc<Shared>,
    peer: [u8; 32],
    pub(super) policy: TerminalPolicy,
}
impl Drop for Reservation {
    fn drop(&mut self) {
        let mut state = lock(&self.shared.state);
        if let Some(count) = state.terminal_pending.get_mut(&self.peer) {
            *count -= 1;
            if *count == 0 {
                state.terminal_pending.remove(&self.peer);
            }
        }
    }
}
pub(super) fn reserve(
    shared: Arc<Shared>,
    peer: [u8; 32],
    persistent: bool,
) -> Result<Reservation, Error> {
    let policy;
    {
        let mut state = lock(&shared.state);
        if state.shut_down {
            return Err(Error::NoSession);
        }
        if !state
            .trust
            .service_granted(&peer, crate::ServiceKind::Terminal)
        {
            return Err(Error::Unauthorized);
        }
        policy = state.terminal_policy;
        if persistent && !policy.allow_persistence {
            return Err(Error::Unauthorized);
        }
        let sessions = lock(&shared.terminal_sessions);
        let pending: usize = state.terminal_pending.values().sum();
        let peer_pending = state.terminal_pending.get(&peer).copied().unwrap_or(0);
        let peer_live = sessions.values().filter(|shell| shell.peer == peer).count();
        if sessions.len() + pending >= usize::from(policy.max_sessions)
            || peer_live + peer_pending >= usize::from(policy.max_sessions_per_peer)
        {
            return Err(Error::Busy);
        }
        drop(sessions);
        *state.terminal_pending.entry(peer).or_default() += 1;
    }
    Ok(Reservation {
        shared,
        peer,
        policy,
    })
}

#[cfg(test)]
mod policy_tests {
    use super::*;
    #[test]
    fn configured_history_replay_and_expiry_have_exact_bounds() {
        let (commands, _rx) = std::sync::mpsc::sync_channel(1);
        let (changed, _) = tokio::sync::watch::channel(0);
        let mut log = Log {
            bytes: VecDeque::new(),
            offset: 0,
            ended: None,
        };
        log.push(vec![1; TERMINAL_CHUNK], 2 * TERMINAL_CHUNK);
        log.push(vec![2; TERMINAL_CHUNK], 2 * TERMINAL_CHUNK);
        log.push(vec![3; TERMINAL_CHUNK], 2 * TERMINAL_CHUNK);
        assert_eq!(log.offset, TERMINAL_CHUNK as u64);
        assert_eq!(log.bytes.len(), 2 * TERMINAL_CHUNK);
        let at = Instant::now();
        let terminal = HostedTerminal {
            peer: [0; 32],
            closed_by_viewer: AtomicBool::new(false),
            info: Mutex::new(TerminalSessionInfo {
                id: "test".into(),
                process_id: 0,
                shell: "test".into(),
                size: TerminalSize {
                    rows: 24,
                    columns: 80,
                },
                persistent: true,
                attached: false,
            }),
            policy: TerminalPolicy {
                history_bytes: (2 * TERMINAL_CHUNK) as u32,
                replay_bytes: TERMINAL_CHUNK as u32,
                detached_timeout_secs: 17,
                ..Default::default()
            },
            commands,
            stop: Arc::new(AtomicBool::new(false)),
            changed,
            log: Mutex::new(log),
            detached: Mutex::new(Some(at)),
        };
        assert_eq!(terminal.replay_start(), (2 * TERMINAL_CHUNK) as u64);
        let (offset, bytes, ended) = terminal.read(terminal.replay_start());
        assert_eq!(offset, (2 * TERMINAL_CHUNK) as u64);
        assert_eq!(bytes, vec![3; TERMINAL_CHUNK]);
        assert_eq!(ended, None);
        assert!(!terminal.detached_expired(at + Duration::from_secs(17) - Duration::from_nanos(1)));
        assert!(terminal.detached_expired(at + Duration::from_secs(17)));
        *lock(&terminal.detached) = None;
        assert!(!terminal.detached_expired(at + Duration::from_secs(100)));
    }
}
