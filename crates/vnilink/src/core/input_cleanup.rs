//! One bounded native cleanup worker per Core. Jobs retain their owner's
//! admission slot until release completes, including after record removal.
use super::*;
type Job = Box<dyn FnOnce() + Send>;
const CAPACITY: usize = 32; // 16 scoped Desktop slots plus at most 4 legacy peers.
pub(super) struct Executor(std::sync::mpsc::SyncSender<Job>);
impl Executor {
    pub(super) fn new() -> std::io::Result<Self> {
        let (send, receive) = std::sync::mpsc::sync_channel::<Job>(CAPACITY);
        std::thread::Builder::new()
            .name("vnilink-input-cleanup".into())
            .spawn(move || {
                while let Ok(job) = receive.recv() {
                    // A backend panic must not kill the only cleanup executor.
                    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(job));
                }
            })?;
        Ok(Self(send))
    }
    pub(super) fn submit(&self, job: impl FnOnce() + Send + 'static) {
        // Admission slots/coalescing bound outstanding jobs below capacity;
        // submitting never blocks Core state or creates another worker.
        if self.0.try_send(Box::new(job)).is_err() {
            panic!("native cleanup admission invariant violated");
        }
    }
}
pub(super) struct AdmissionSlot(pub(super) Arc<AtomicUsize>);
impl Drop for AdmissionSlot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
    }
}
/// Settle bookkeeping even if a native backend panics during cleanup.
pub(super) struct Reservation {
    pub(super) pending: Arc<AtomicUsize>,
    pub(super) releasing: Option<Arc<AtomicBool>>,
}
impl Drop for Reservation {
    fn drop(&mut self) {
        if let Some(releasing) = &self.releasing {
            releasing.store(false, std::sync::atomic::Ordering::Release);
        }
        self.pending
            .fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
    }
}
