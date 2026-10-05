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
        Self::with_state(name, || (), move |_, query| run(query))
    }
    /// Initializes thread-confined service state inside the worker; only queries/replies need Send.
    pub fn with_state<S: 'static>(
        name: &str,
        initialize: impl FnOnce() -> S + Send + 'static,
        mut run: impl FnMut(&mut S, Q) -> R + Send + 'static,
    ) -> Self {
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
                let mut state = initialize();
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
                    let reply = run(&mut state, query);
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
    /// A non-Send Rc service is constructed and retained on its worker, never transferred from UI.
    fn state_is_initialized_on_its_own_thread() {
        let owner = std::thread::current().id();
        let (reply_tx, reply_rx) = std::sync::mpsc::channel();
        let worker = LatestWorker::with_state(
            "confined-state",
            move || {
                assert_ne!(std::thread::current().id(), owner);
                std::rc::Rc::new(std::cell::Cell::new(0usize))
            },
            move |state, amount| {
                state.set(state.get() + amount);
                reply_tx.send(state.get()).unwrap();
                state.get()
            },
        );
        worker.submit(1, 3);
        assert_eq!(
            reply_rx
                .recv_timeout(std::time::Duration::from_secs(2))
                .unwrap(),
            3
        );
        worker.submit(2, 4);
        assert_eq!(
            reply_rx
                .recv_timeout(std::time::Duration::from_secs(2))
                .unwrap(),
            7
        );
    }
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
