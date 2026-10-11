//! Cancellable USB worker; slow helper calls never run on the pose or control poll thread.
use crate::{UsbDeviceIdentity, UsbTransferBackend};
use light_bridge_core::{Error, Result};
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError},
        Arc,
    },
    thread::{self, JoinHandle},
    time::Duration,
};
pub trait ManagedUsb: UsbTransferBackend {
    /// Implementations must observe this flag during all waits, including connection setup.
    fn set_cancellation(&mut self, cancel: Arc<AtomicBool>);
}
#[cfg(target_os = "linux")]
impl ManagedUsb for crate::helper::KernelUsbIp {
    fn set_cancellation(&mut self, cancel: Arc<AtomicBool>) {
        self.set_cancel(cancel);
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Export,
    Attach,
    Detach,
    Unexport,
}
pub enum Task {
    Export(u64, UsbDeviceIdentity),
    Attach(u64, UsbDeviceIdentity),
    Detach(u64, u32),
    Unexport(u64),
}
pub struct Completion {
    pub session: u64,
    pub kind: Kind,
    pub result: Result<Option<u32>>,
}
pub struct UsbManager {
    tx: Option<SyncSender<Task>>,
    rx: Receiver<Completion>,
    cancel: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}
impl UsbManager {
    pub fn new(mut backend: Box<dyn ManagedUsb>) -> Self {
        let cancel = Arc::new(AtomicBool::new(false));
        backend.set_cancellation(cancel.clone());
        let (tx, commands) = mpsc::sync_channel(4);
        let (replies, rx) = mpsc::sync_channel(4);
        let c = cancel.clone();
        let worker = thread::spawn(move || {
            while !c.load(Ordering::Acquire) {
                let task = match commands.recv_timeout(Duration::from_millis(2)) {
                    Ok(t) => t,
                    Err(mpsc::RecvTimeoutError::Timeout) => continue,
                    Err(_) => break,
                };
                let (session, kind, result) = match task {
                    Task::Export(s, d) => (s, Kind::Export, backend.export(s, &d).map(|_| None)),
                    Task::Attach(s, d) => (s, Kind::Attach, backend.attach(s, &d).map(Some)),
                    Task::Detach(s, p) => (s, Kind::Detach, backend.detach(s, p).map(|_| None)),
                    Task::Unexport(s) => (s, Kind::Unexport, backend.unexport(s).map(|_| None)),
                };
                if replies
                    .try_send(Completion {
                        session,
                        kind,
                        result,
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
        Self {
            tx: Some(tx),
            rx,
            cancel,
            worker: Some(worker),
        }
    }
    pub fn submit(&self, task: Task) -> Result<()> {
        self.tx
            .as_ref()
            .ok_or(Error::Disconnected)?
            .try_send(task)
            .map_err(|e| match e {
                TrySendError::Full(_) => Error::Backpressure,
                TrySendError::Disconnected(_) => Error::Disconnected,
            })
    }
    pub fn poll(&self) -> Result<Option<Completion>> {
        match self.rx.try_recv() {
            Ok(c) => Ok(Some(c)),
            Err(TryRecvError::Empty) => Ok(None),
            Err(_) => Err(Error::Disconnected),
        }
    }
}
impl Drop for UsbManager {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
        self.tx.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
