//! A bounded worker: one running request, one replaceable pending request, one reply.
use std::sync::{Arc, Condvar, Mutex};

struct Mailbox<Q, R> {
    pending: Option<(u64, Q)>,
    reply: Option<(u64, R)>,
    stop: bool,
}
pub struct LatestWorker<Q, R> {
    shared: Arc<(Mutex<Mailbox<Q, R>>, Condvar)>,
}

impl<Q: Send + 'static, R: Send + 'static> LatestWorker<Q, R> {
    /// Runs blocking service work on one named thread; rapid requests are coalesced.
    pub fn new(name: &str, mut run: impl FnMut(Q) -> R + Send + 'static) -> Self {
        let shared = Arc::new((
            Mutex::new(Mailbox {
                pending: None,
                reply: None,
                stop: false,
            }),
            Condvar::new(),
        ));
        let thread_shared = shared.clone();
        std::thread::Builder::new()
            .name(name.into())
            .spawn(move || {
                loop {
                    let request = {
                        let (mutex, signal) = &*thread_shared;
                        let mut mailbox = mutex.lock().expect("worker mailbox");
                        while mailbox.pending.is_none() && !mailbox.stop {
                            mailbox = signal.wait(mailbox).expect("worker mailbox");
                        }
                        if mailbox.stop {
                            break;
                        }
                        mailbox.pending.take().expect("pending request")
                    };
                    let (generation, query) = request;
                    let reply = run(query);
                    let mut mailbox = thread_shared.0.lock().expect("worker mailbox");
                    mailbox.reply = Some((generation, reply));
                }
            })
            .expect("start worker");
        Self { shared }
    }
    /// Replaces queued work; never accumulates unbounded file-selection requests.
    pub fn submit(&self, generation: u64, query: Q) {
        self.shared.0.lock().expect("worker mailbox").pending = Some((generation, query));
        self.shared.1.notify_one();
    }
    /// Takes a completed reply; the UI checks its generation before applying it.
    pub fn take(&self) -> Option<(u64, R)> {
        self.shared.0.lock().expect("worker mailbox").reply.take()
    }
}
impl<Q, R> Drop for LatestWorker<Q, R> {
    /// Stops queued work without blocking the UI while an active read finishes.
    fn drop(&mut self) {
        if let Ok(mut mailbox) = self.shared.0.lock() {
            mailbox.stop = true;
            mailbox.pending = None;
        }
        self.shared.1.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    /// A blocked running request leaves room for only the newest pending selection.
    fn replaces_pending_requests() {
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (continue_tx, continue_rx) = std::sync::mpsc::channel();
        let (result_tx, result_rx) = std::sync::mpsc::channel();
        let worker = LatestWorker::new("test-worker", move |value| {
            if value == 1 {
                started_tx.send(()).unwrap();
                continue_rx.recv().unwrap();
            }
            result_tx.send(value).unwrap();
            value
        });
        worker.submit(1, 1);
        started_rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap();
        worker.submit(2, 2);
        worker.submit(3, 3);
        continue_tx.send(()).unwrap();
        assert_eq!(
            result_rx
                .recv_timeout(std::time::Duration::from_secs(2))
                .unwrap(),
            1
        );
        assert_eq!(
            result_rx
                .recv_timeout(std::time::Duration::from_secs(2))
                .unwrap(),
            3
        );
        assert!(result_rx.try_recv().is_err());
    }
}
