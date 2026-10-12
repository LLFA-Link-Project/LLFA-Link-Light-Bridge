//! Bounded, cancellable automatic helper worker. No OS cleanup on Drop.
use crate::{
    bootstrap_rpc::{self, Envelope, Response},
    kernel::KResult,
};
use std::{
    os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd},
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError},
        Arc,
    },
    thread::{self, JoinHandle},
    time::Duration,
};
const CAPACITY: usize = 4;
pub trait Backend: Send + 'static {
    fn transact(
        &mut self,
        request: Envelope,
        fd: Option<RawFd>,
        cancel: &AtomicBool,
    ) -> KResult<Response>;
}
struct Remote(PathBuf);
impl Backend for Remote {
    fn transact(
        &mut self,
        request: Envelope,
        fd: Option<RawFd>,
        cancel: &AtomicBool,
    ) -> KResult<Response> {
        bootstrap_rpc::call_cancelable(&self.0, request, fd, cancel)
    }
}
struct Job {
    id: u64,
    request: Envelope,
    fd: Option<OwnedFd>,
}
pub struct Worker {
    tx: SyncSender<Job>,
    rx: Receiver<(u64, KResult<Response>)>,
    cancel: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}
impl Worker {
    pub fn new(socket: PathBuf) -> KResult<Self> {
        if !socket.is_absolute() {
            return Err("absolute automatic helper socket required".into());
        }
        Ok(Self::with_backend(Remote(socket)))
    }
    pub fn with_backend(mut backend: impl Backend) -> Self {
        let (tx, jobs) = mpsc::sync_channel::<Job>(CAPACITY);
        let (results, rx) = mpsc::sync_channel(CAPACITY);
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = cancel.clone();
        let join = thread::spawn(move || {
            while !flag.load(Ordering::Acquire) {
                let job = match jobs.recv_timeout(Duration::from_millis(2)) {
                    Ok(job) => job,
                    Err(mpsc::RecvTimeoutError::Timeout) => continue,
                    Err(_) => break,
                };
                let response =
                    backend.transact(job.request, job.fd.as_ref().map(AsRawFd::as_raw_fd), &flag);
                let mut result = (job.id, response);
                loop {
                    if flag.load(Ordering::Acquire) {
                        return;
                    }
                    match results.try_send(result) {
                        Ok(()) => break,
                        Err(TrySendError::Full(value)) => {
                            result = value;
                            thread::sleep(Duration::from_millis(1));
                        }
                        Err(TrySendError::Disconnected(_)) => return,
                    }
                }
            }
        });
        Self {
            tx,
            rx,
            cancel,
            join: Some(join),
        }
    }
    pub fn submit(&self, id: u64, request: Envelope, route: Option<RawFd>) -> KResult<()> {
        if id == 0 || self.cancel.load(Ordering::Acquire) {
            return Err("worker cancelled/zero operation ID".into());
        }
        request.validate()?;
        if matches!(
            request.action,
            bootstrap_rpc::Action::PrepareExport { .. }
                | bootstrap_rpc::Action::PrepareImport { .. }
        ) != route.is_some()
        {
            return Err("Prepare requires a route descriptor; other operations forbid it".into());
        }
        let fd = if let Some(fd) = route {
            let raw = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 0) };
            if raw < 0 {
                return Err("invalid route FD".into());
            }
            Some(unsafe { OwnedFd::from_raw_fd(raw) })
        } else {
            None
        };
        self.tx
            .try_send(Job { id, request, fd })
            .map_err(|e| match e {
                TrySendError::Full(_) => "worker queue full".into(),
                TrySendError::Disconnected(_) => "worker disconnected".into(),
            })
    }
    pub fn poll(&self) -> KResult<Option<(u64, KResult<Response>)>> {
        match self.rx.try_recv() {
            Ok(v) => Ok(Some(v)),
            Err(TryRecvError::Empty) => Ok(None),
            Err(_) => Err("worker disconnected".into()),
        }
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}
