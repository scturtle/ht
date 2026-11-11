//! Bounded pipe writes. A timeout is uncertain delivery, never permission to retry.
use std::{
    collections::VecDeque,
    io::{self, Write},
    sync::mpsc::{self, Receiver, SyncSender},
    time::{Duration, Instant},
};

const MAX_IN_FLIGHT: usize = 64;
enum Payload {
    Inline { bytes: [u8; 32], len: usize },
    Large(Vec<u8>),
}
impl Payload {
    fn new(data: &[u8]) -> Self {
        if data.len() <= 32 {
            let mut bytes = [0; 32];
            bytes[..data.len()].copy_from_slice(data);
            Self::Inline {
                bytes,
                len: data.len(),
            }
        } else {
            Self::Large(data.to_vec())
        }
    }
    fn bytes(&self) -> &[u8] {
        match self {
            Self::Inline { bytes, len } => &bytes[..*len],
            Self::Large(data) => data,
        }
    }
}
struct WriteJob {
    header: [u8; 9],
    payload: Payload,
}
struct Completion {
    at: Instant,
    result: io::Result<()>,
}

pub(super) struct FrameWriter {
    sender: SyncSender<WriteJob>,
    replies: Receiver<Completion>,
    deadlines: VecDeque<Instant>,
    failed: bool,
}
impl FrameWriter {
    pub fn new(mut pipe: impl Write + Send + 'static) -> io::Result<Self> {
        let (sender, receiver) = mpsc::sync_channel::<WriteJob>(MAX_IN_FLIGHT);
        let (complete, replies) = mpsc::sync_channel(MAX_IN_FLIGHT);
        std::thread::Builder::new()
            .name("core-frame-writer".into())
            .spawn(move || {
                for job in receiver {
                    let result = pipe
                        .write_all(&job.header)
                        .and_then(|_| pipe.write_all(job.payload.bytes()))
                        .and_then(|_| pipe.flush());
                    let failed = result.is_err();
                    if complete
                        .send(Completion {
                            at: Instant::now(),
                            result,
                        })
                        .is_err()
                        || failed
                    {
                        break;
                    }
                }
            })?;
        Ok(Self {
            sender,
            replies,
            deadlines: VecDeque::new(),
            failed: false,
        })
    }
    fn poisoned() -> io::Error {
        io::Error::new(
            io::ErrorKind::BrokenPipe,
            "Core writer failed; reconnect required, input was not retried",
        )
    }
    fn timeout() -> io::Error {
        io::Error::new(
            io::ErrorKind::TimedOut,
            "Core write timed out; delivery is uncertain and will not be retried",
        )
    }
    fn accept(&mut self, reply: Completion) -> io::Result<()> {
        let deadline = self.deadlines.pop_front().ok_or_else(Self::poisoned)?;
        if reply.at > deadline {
            return Err(Self::timeout());
        }
        reply.result
    }
    fn fail<T>(&mut self, error: io::Error) -> io::Result<T> {
        self.failed = true;
        Err(error)
    }
    pub fn health(&mut self) -> io::Result<()> {
        if self.failed {
            return Err(Self::poisoned());
        }
        while !self.deadlines.is_empty() {
            match self.replies.try_recv() {
                Ok(reply) => {
                    if let Err(error) = self.accept(reply) {
                        return self.fail(error);
                    }
                }
                Err(mpsc::TryRecvError::Disconnected) => return self.fail(Self::poisoned()),
                Err(mpsc::TryRecvError::Empty) => break,
            }
        }
        if self
            .deadlines
            .front()
            .is_some_and(|deadline| *deadline <= Instant::now())
        {
            return self.fail(Self::timeout());
        }
        Ok(())
    }
    fn wait_one(&mut self, limit: Instant) -> io::Result<()> {
        let Some(deadline) = self.deadlines.front().copied() else {
            return Ok(());
        };
        let deadline = deadline.min(limit);
        let result = self
            .replies
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .map_err(|error| match error {
                mpsc::RecvTimeoutError::Timeout => Self::timeout(),
                mpsc::RecvTimeoutError::Disconnected => Self::poisoned(),
            })
            .and_then(|reply| {
                if reply.at > limit {
                    return Err(Self::timeout());
                }
                self.accept(reply)
            });
        if let Err(error) = result {
            return self.fail(error);
        }
        Ok(())
    }
    fn enqueue(
        &mut self,
        channel: u32,
        kind: u8,
        payload: &[u8],
        timeout: Duration,
    ) -> io::Result<()> {
        self.health()?;
        let deadline = Instant::now() + timeout;
        if self.deadlines.len() == MAX_IN_FLIGHT {
            self.wait_one(deadline)?;
        }
        if Instant::now() >= deadline {
            return self.fail(Self::timeout());
        }
        let length = u32::try_from(payload.len())
            .map_err(|_| io::Error::other("Core payload length exceeds u32"))?;
        let mut header = [0; 9];
        header[..4].copy_from_slice(&channel.to_le_bytes());
        header[4] = kind;
        header[5..].copy_from_slice(&length.to_le_bytes());
        if self
            .sender
            .try_send(WriteJob {
                header,
                payload: Payload::new(payload),
            })
            .is_err()
        {
            return self.fail(Self::poisoned());
        }
        self.deadlines.push_back(deadline);
        Ok(())
    }
    pub fn send(
        &mut self,
        channel: u32,
        kind: u8,
        payload: &[u8],
        timeout: Duration,
    ) -> io::Result<()> {
        let deadline = Instant::now() + timeout;
        self.enqueue(
            channel,
            kind,
            payload,
            deadline.saturating_duration_since(Instant::now()),
        )?;
        while !self.deadlines.is_empty() {
            self.wait_one(deadline)?;
        }
        Ok(())
    }
    /// Credits are scalar notifications, not input acknowledgements. Queue them
    /// without a per-chunk thread round trip; send()/health() retain FIFO errors
    /// and deadlines. At most 64 inline jobs, plus one synchronous large frame.
    pub fn notify_u32(
        &mut self,
        channel: u32,
        kind: u8,
        bytes: u32,
        timeout: Duration,
    ) -> io::Result<()> {
        self.enqueue(channel, kind, &bytes.to_le_bytes(), timeout)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    struct Capture(Arc<Mutex<Vec<u8>>>);
    impl Write for Capture {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn complete_frames_and_flushes_preserve_binary_order() {
        let bytes = Arc::new(Mutex::new(Vec::new()));
        let mut writer = FrameWriter::new(Capture(bytes.clone())).unwrap();
        writer
            .send(7, 0, &[0, 255, 27], Duration::from_secs(1))
            .unwrap();
        writer
            .send(7, 6, b"answer", Duration::from_secs(1))
            .unwrap();
        let bytes = bytes.lock().unwrap();
        let mut expected = Vec::new();
        for (kind, payload) in [(0, &[0, 255, 27][..]), (6, b"answer")] {
            expected.extend(7u32.to_le_bytes());
            expected.push(kind);
            expected.extend((payload.len() as u32).to_le_bytes());
            expected.extend(payload);
        }
        assert_eq!(*bytes, expected);
    }

    struct Stalled {
        release: mpsc::Receiver<()>,
        captured: Arc<Mutex<Vec<u8>>>,
        finished: mpsc::Sender<()>,
    }
    impl Write for Stalled {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if self.captured.lock().unwrap().is_empty() {
                self.release.recv().unwrap();
            }
            self.captured.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            self.finished.send(()).unwrap();
            Ok(())
        }
    }

    #[test]
    fn timed_out_partial_delivery_is_never_reused_or_replayed() {
        let (release, blocked) = mpsc::channel();
        let (finished, done) = mpsc::channel();
        let captured = Arc::new(Mutex::new(Vec::new()));
        let mut writer = FrameWriter::new(Stalled {
            release: blocked,
            captured: captured.clone(),
            finished,
        })
        .unwrap();
        assert_eq!(
            writer
                .send(1, 0, b"first", Duration::from_millis(10))
                .unwrap_err()
                .kind(),
            io::ErrorKind::TimedOut
        );
        assert_eq!(
            writer
                .send(1, 0, b"second", Duration::from_secs(1))
                .unwrap_err()
                .kind(),
            io::ErrorKind::BrokenPipe
        );
        release.send(()).unwrap();
        done.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_eq!(&captured.lock().unwrap()[9..], b"first");
        // Even a late completion cannot un-fence the failed connection.
        assert!(writer.send(1, 0, b"third", Duration::from_secs(1)).is_err());
    }

    #[test]
    fn queued_scalar_controls_stay_ordered_before_synchronous_input() {
        let bytes = Arc::new(Mutex::new(Vec::new()));
        let mut writer = FrameWriter::new(Capture(bytes.clone())).unwrap();
        writer
            .notify_u32(7, 1, 4096, Duration::from_secs(1))
            .unwrap();
        writer
            .notify_u32(8, 1, 8192, Duration::from_secs(1))
            .unwrap();
        writer
            .send(7, 6, b"answer", Duration::from_secs(1))
            .unwrap();
        let bytes = bytes.lock().unwrap();
        assert_eq!(&bytes[..4], &7u32.to_le_bytes());
        assert_eq!(&bytes[9..13], &4096u32.to_le_bytes());
        assert_eq!(&bytes[13..17], &8u32.to_le_bytes());
        assert_eq!(&bytes[22..26], &8192u32.to_le_bytes());
        assert_eq!(&bytes[35..], b"answer");
    }

    #[test]
    fn asynchronous_credit_deadline_is_checked_even_without_another_write() {
        let (release, blocked) = mpsc::channel();
        let (finished, done) = mpsc::channel();
        let mut writer = FrameWriter::new(Stalled {
            release: blocked,
            captured: Arc::new(Mutex::new(Vec::new())),
            finished,
        })
        .unwrap();
        writer
            .notify_u32(1, 1, 123, Duration::from_secs(1))
            .unwrap();
        writer.deadlines[0] = Instant::now() - Duration::from_secs(1);
        assert_eq!(writer.health().unwrap_err().kind(), io::ErrorKind::TimedOut);
        release.send(()).unwrap();
        done.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_eq!(
            writer.health().unwrap_err().kind(),
            io::ErrorKind::BrokenPipe
        );
    }

    #[test]
    fn stalled_control_queue_is_bounded_and_never_accepts_a_new_input_after_failure() {
        let (release, blocked) = mpsc::channel();
        let (finished, done) = mpsc::channel();
        let captured = Arc::new(Mutex::new(Vec::new()));
        let mut writer = FrameWriter::new(Stalled {
            release: blocked,
            captured: captured.clone(),
            finished,
        })
        .unwrap();
        for _ in 0..MAX_IN_FLIGHT {
            writer.notify_u32(1, 1, 1, Duration::from_secs(1)).unwrap();
        }
        assert_eq!(writer.deadlines.len(), MAX_IN_FLIGHT);
        writer.deadlines[0] = Instant::now() - Duration::from_secs(1);
        assert!(writer
            .send(1, 0, b"never sent", Duration::from_secs(1))
            .is_err());
        release.send(()).unwrap();
        for _ in 0..MAX_IN_FLIGHT {
            done.recv_timeout(Duration::from_secs(1)).unwrap();
        }
        assert_eq!(captured.lock().unwrap().len(), MAX_IN_FLIGHT * 13);
    }

    #[test]
    fn blocked_pipe_child_fixture() {
        if std::env::var_os("HERDR_TEST_BLOCKED_CORE_PIPE").is_some() {
            println!("BLOCKED_CORE_PIPE_READY");
            io::stdout().flush().unwrap();
            loop {
                std::thread::park();
            }
        }
    }

    #[test]
    fn real_child_pipe_timeout_and_owned_child_shutdown_do_not_hang() {
        use std::io::BufRead;
        use std::process::{Child, Stdio};
        struct OwnedChild(Child);
        impl Drop for OwnedChild {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
        let mut child = OwnedChild(
            crate::noninteractive_process::command(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "client::core::writer::tests::blocked_pipe_child_fixture",
                    "--nocapture",
                ])
                .env("HERDR_TEST_BLOCKED_CORE_PIPE", "1")
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        );
        let mut output = io::BufReader::new(child.0.stdout.take().unwrap());
        loop {
            let mut line = String::new();
            assert!(
                output.read_line(&mut line).unwrap() > 0,
                "child fixture exited before readiness"
            );
            if line.contains("BLOCKED_CORE_PIPE_READY") {
                break;
            }
        }
        let mut writer = FrameWriter::new(child.0.stdin.take().unwrap()).unwrap();
        let error = writer
            .send(1, 0, &vec![0; 4 << 20], Duration::from_millis(50))
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        // Kill only this test child, as Core::drop does for its stdio relay.
        drop(child);
        assert_eq!(
            writer
                .send(1, 0, b"not replayed", Duration::from_secs(1))
                .unwrap_err()
                .kind(),
            io::ErrorKind::BrokenPipe
        );
    }

    struct Failed;
    impl Write for Failed {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "test pipe closed",
            ))
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    #[test]
    fn pipe_error_fences_subsequent_writes() {
        let mut writer = FrameWriter::new(Failed).unwrap();
        for _ in 0..2 {
            assert_eq!(
                writer
                    .send(1, 0, b"input", Duration::from_secs(1))
                    .unwrap_err()
                    .kind(),
                io::ErrorKind::BrokenPipe
            );
        }
    }
}
