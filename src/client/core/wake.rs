//! A single-owner worker wake shared by command, reader and writer producers.
//! `unpark` retains one token: enqueue-before-park cannot lose the notification.
use std::{
    sync::OnceLock,
    thread::{self, Thread},
    time::Instant,
};
#[derive(Default)]
pub(super) struct Wake {
    worker: OnceLock<Thread>,
}
impl Wake {
    pub(super) fn bind_current(&self) {
        self.worker.get_or_init(thread::current);
    }
    pub(super) fn notify(&self) {
        if let Some(worker) = self.worker.get() {
            worker.unpark();
        }
    }
    pub(super) fn wait(&self, deadline: Option<Instant>) {
        match deadline {
            Some(deadline) => {
                thread::park_timeout(deadline.saturating_duration_since(Instant::now()))
            }
            None => thread::park(),
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        sync::{mpsc, Arc},
        time::Duration,
    };
    #[test]
    fn notification_before_wait_is_retained_and_an_idle_worker_does_not_tick() {
        let wake = Arc::new(Wake::default());
        let child_wake = wake.clone();
        let (ready_tx, ready) = mpsc::channel();
        let (done_tx, done) = mpsc::channel();
        let go = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let child_go = go.clone();
        let child = thread::spawn(move || {
            child_wake.bind_current();
            ready_tx.send(()).unwrap();
            // Do not insert another blocking primitive between notify and park:
            // std channel waits may legitimately consume the thread's park token.
            while !child_go.load(std::sync::atomic::Ordering::Acquire) {
                std::hint::spin_loop();
            }
            let start = Instant::now();
            child_wake.wait(Some(start + Duration::from_secs(1)));
            done_tx.send(start.elapsed()).unwrap();
            child_wake.wait(None);
            done_tx.send(Duration::ZERO).unwrap();
        });
        ready.recv().unwrap();
        wake.notify();
        go.store(true, std::sync::atomic::Ordering::Release);
        assert!(done.recv_timeout(Duration::from_secs(2)).unwrap() < Duration::from_millis(500));
        assert!(
            done.recv_timeout(Duration::from_millis(60)).is_err(),
            "idle wait must not poll"
        );
        wake.notify();
        done.recv_timeout(Duration::from_secs(1)).unwrap();
        child.join().unwrap();
    }
    #[test]
    fn a_deadline_wakes_without_another_event() {
        let wake = Wake::default();
        wake.bind_current();
        let start = Instant::now();
        let deadline = start + Duration::from_millis(30);
        wake.wait(Some(deadline));
        assert!(Instant::now() >= deadline);
    }
}
