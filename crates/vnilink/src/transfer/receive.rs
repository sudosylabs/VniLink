//! Asking for one file and receiving it into its partial.

use std::time::Duration;

use iroh::endpoint::{Connection, ReadError, ReadExactError, RecvStream, VarInt};

use crate::error::Error;

use super::wire::{
    FileRequest, RESET_CHANGED, RESET_STALLED, STATUS_BUSY, STATUS_CHANGED, STATUS_SENDING,
    VALUE_LEN,
};
use super::{Partial, TransferCounters, CHUNK, STALL_WITHIN};

/// How long the sender may take to answer a request. Short in tests,
/// which hold a sender back.
const ANSWER_WITHIN: Duration = if cfg!(test) {
    Duration::from_secs(1)
} else {
    Duration::from_secs(15)
};

/// The sender's answer to a request.
pub(crate) enum Answer {
    /// The bytes follow on this stream.
    Sending(RecvStream),
    /// The source is no longer the file the manifest describes.
    Changed,
    /// The sender does not serve this file now.
    Denied,
    /// The sender serves as many files as it allows; ask again later.
    Busy,
}

/// How receiving one file ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Received {
    /// Every segment of the file checked; carries the bytes received,
    /// below the size when it resumed.
    Done { downloaded: u64 },
    /// Stopped by the cancel watch; the partial stays for resume.
    Cancelled,
    /// The sender does not serve this file now.
    Denied,
    /// The sender serves as many files as it allows; ask again later.
    Busy,
    /// The source is no longer the file the manifest describes.
    Changed,
    /// No byte arrived in time; the stream is stopped and the partial keeps
    /// its checked segments.
    Stalled,
    /// A segment did not hash to the sender's value; its bytes are
    /// dropped and the partial keeps the segments before it.
    Mismatch,
}

/// Opens a stream, sends the request and reads the sender's answer, which
/// must come within [`ANSWER_WITHIN`].
pub(crate) async fn ask(connection: &Connection, request: FileRequest) -> Result<Answer, Error> {
    tokio::time::timeout(ANSWER_WITHIN, ask_now(connection, request))
        .await
        .map_err(|_| Error::Timeout)?
}

async fn ask_now(connection: &Connection, request: FileRequest) -> Result<Answer, Error> {
    let (mut send, mut recv) = connection.open_bi().await.map_err(Error::network)?;
    send.write_all(&request.encode())
        .await
        .map_err(Error::network)?;
    send.finish().map_err(Error::network)?;
    let mut status = [0u8; 1];
    // A reset discards bytes not yet delivered, the status byte among
    // them: a sender that found its source changed at once says so only
    // by the reset code.
    match recv.read_exact(&mut status).await {
        Ok(()) => {}
        Err(ReadExactError::ReadError(ReadError::Reset(code)))
            if code == VarInt::from_u32(RESET_CHANGED) =>
        {
            return Ok(Answer::Changed);
        }
        Err(error) => return Err(Error::network(error)),
    }
    Ok(match status[0] {
        STATUS_SENDING => Answer::Sending(recv),
        STATUS_CHANGED => Answer::Changed,
        STATUS_BUSY => Answer::Busy,
        _ => Answer::Denied,
    })
}

/// Appends what arrives on `recv` to `partial` until the file is whole,
/// checking each segment against the value that follows it. Reports the
/// bytes written to `progress` as they arrive; a cancel stops at once and
/// keeps the checked segments, and so does a sender that sends nothing for
/// [`STALL_WITHIN`].
pub(crate) async fn receive(
    mut recv: RecvStream,
    partial: &mut Partial,
    size: u64,
    progress: &std::sync::mpsc::Sender<u64>,
    counters: &TransferCounters,
    cancel: &mut tokio::sync::watch::Receiver<bool>,
) -> Result<Received, Error> {
    let from = partial.checked();
    let stopped = async {
        if cancel.wait_for(|stop| *stop).await.is_err() {
            std::future::pending::<()>().await;
        }
    };
    tokio::pin!(stopped);
    let mut incoming = Incoming::default();
    loop {
        let chunk = tokio::select! {
            biased;
            _ = &mut stopped => return Ok(Received::Cancelled),
            chunk = tokio::time::timeout(STALL_WITHIN, recv.read_chunk(CHUNK)) => chunk,
        };
        let Ok(chunk) = chunk else {
            let _ = recv.stop(VarInt::from_u32(RESET_STALLED));
            return Ok(Received::Stalled);
        };
        let bytes = match chunk {
            Ok(Some(bytes)) => bytes,
            Ok(None) => break,
            Err(ReadError::Reset(code)) if code == VarInt::from_u32(RESET_CHANGED) => {
                return Ok(Received::Changed);
            }
            Err(error) => return Err(Error::network(error)),
        };
        if !incoming.feed(partial, &bytes, progress, counters)? {
            return Ok(Received::Mismatch);
        }
    }
    incoming.finish(partial, size)?;
    Ok(Received::Done {
        downloaded: size - from,
    })
}

/// Splits the bytes of a sending stream, however they are chunked, into
/// segment bytes for the partial and the value after each segment.
#[derive(Default)]
struct Incoming {
    /// The value of the segment just written, while it arrives.
    value: Option<Vec<u8>>,
}

impl Incoming {
    /// Takes the next bytes of the stream. False when a segment failed its
    /// check; an error when the bytes cannot be part of the file.
    fn feed(
        &mut self,
        partial: &mut Partial,
        mut bytes: &[u8],
        progress: &std::sync::mpsc::Sender<u64>,
        counters: &TransferCounters,
    ) -> Result<bool, Error> {
        while !bytes.is_empty() {
            if let Some(pending) = self.value.as_mut() {
                let take = bytes.len().min(VALUE_LEN - pending.len());
                pending.extend_from_slice(&bytes[..take]);
                bytes = &bytes[take..];
                if pending.len() == VALUE_LEN {
                    let matched = partial.seal(pending)?;
                    counters.checked(matched);
                    if !matched {
                        return Ok(false);
                    }
                    self.value = None;
                }
                continue;
            }
            let left = partial.segment_left();
            if left == 0 {
                // Bytes past the file's end.
                return Err(Error::Malformed);
            }
            let take = bytes.len().min(left as usize);
            partial.append(&bytes[..take])?;
            counters.received(take);
            bytes = &bytes[take..];
            let _ = progress.send(partial.written());
            if partial.segment_left() == 0 {
                self.value = Some(Vec::with_capacity(VALUE_LEN));
            }
        }
        Ok(true)
    }

    /// Checks the stream ended where the file does.
    fn finish(&self, partial: &Partial, size: u64) -> Result<(), Error> {
        if self.value.is_some() || partial.checked() != size {
            return Err(Error::network("the file stream ended early"));
        }
        Ok(())
    }
}

/// What an honest sender sends for `bytes` cut into segments of
/// `segment`: each segment, then its chaining value.
#[cfg(any(test, feature = "fuzzing"))]
pub(crate) fn stream_of(bytes: &[u8], segment: u64) -> Vec<u8> {
    let size = bytes.len() as u64;
    let mut stream = Vec::new();
    let mut start = 0;
    while start < size {
        let end = start + super::hash::segment_len(start, size, segment);
        let mut hasher = super::hash::SegmentHasher::at(start);
        hasher.update(&bytes[start as usize..end as usize]);
        stream.extend_from_slice(&bytes[start as usize..end as usize]);
        stream.extend_from_slice(&hasher.finalize());
        start = end;
    }
    stream
}

/// Feeds `chunks` of a sending stream to a fresh `size`-byte partial at
/// `data` and `journal`, cut into segments of `segment`, as [`receive`]
/// does with what arrives. Ok(true) when the file landed whole, Ok(false)
/// when a segment failed its check.
#[cfg(feature = "fuzzing")]
pub(crate) fn land<'a>(
    data: &std::path::Path,
    journal: &std::path::Path,
    size: u64,
    segment: u64,
    chunks: impl IntoIterator<Item = &'a [u8]>,
) -> Result<bool, Error> {
    let _ = std::fs::remove_file(data);
    let _ = std::fs::remove_file(journal);
    let mut partial = Partial::open_in(data, journal, size, segment)?;
    let mut incoming = Incoming::default();
    let (progress, _) = std::sync::mpsc::channel();
    let counters = TransferCounters::default();
    for chunk in chunks {
        if !incoming.feed(&mut partial, chunk, &progress, &counters)? {
            return Ok(false);
        }
    }
    incoming.finish(&partial, size)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use iroh::endpoint::{presets, SendStream};
    use iroh::{Endpoint, RelayMode};

    use super::super::hash::SegmentHasher;
    use super::super::wire::{FILES_ALPN, STATUS_DENIED};
    use super::*;

    async fn endpoint() -> Endpoint {
        Endpoint::builder(presets::N0)
            .alpns(vec![FILES_ALPN.to_vec()])
            .relay_mode(RelayMode::Disabled)
            .clear_address_lookup()
            .clear_ip_transports()
            .bind_addr("127.0.0.1:0")
            .unwrap()
            .bind()
            .await
            .unwrap()
    }

    /// A receiving connection and the sending one it reached. The
    /// endpoints are returned so they outlive the test body.
    async fn pair() -> (Connection, Connection, Endpoint, Endpoint) {
        let sender = endpoint().await;
        let receiver = endpoint().await;
        let accepting = sender.clone();
        let accepted =
            tokio::spawn(async move { accepting.accept().await.unwrap().await.unwrap() });
        let connection = receiver.connect(sender.addr(), FILES_ALPN).await.unwrap();
        (connection, accepted.await.unwrap(), receiver, sender)
    }

    struct Scratch(std::path::PathBuf);

    impl Scratch {
        fn new(tag: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("vnilink-receive-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn partial(&self, size: u64) -> Partial {
            Partial::open(&self.0.join("data"), &self.0.join("journal"), size).unwrap()
        }

        fn data(&self) -> Vec<u8> {
            std::fs::read(self.0.join("data")).unwrap()
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn value_of(bytes: &[u8]) -> [u8; 32] {
        let mut hasher = SegmentHasher::at(0);
        hasher.update(bytes);
        hasher.finalize()
    }

    /// Receives a `size`-byte file from a sender that answers the request
    /// with `script`, run against the raw stream after the status byte.
    async fn receive_from<F, Fut>(
        scratch: &Scratch,
        size: u64,
        script: F,
    ) -> (Result<Received, Error>, Partial)
    where
        F: FnOnce(SendStream) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = ()> + Send,
    {
        let (received, partial, _) = receive_timed(scratch, size, script).await;
        (received, partial)
    }

    /// [`receive_from`], with how long the receiving took once the stream
    /// was answered: the loopback connection's setup is not part of it.
    async fn receive_timed<F, Fut>(
        scratch: &Scratch,
        size: u64,
        script: F,
    ) -> (Result<Received, Error>, Partial, Duration)
    where
        F: FnOnce(SendStream) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = ()> + Send,
    {
        let (connection, serving, _receiver, _sender) = pair().await;
        tokio::spawn(async move {
            let (mut send, mut recv) = serving.accept_bi().await.unwrap();
            super::super::read_request(&mut recv).await.unwrap();
            send.write_all(&[STATUS_SENDING]).await.unwrap();
            script(send).await;
            // Keep the connection until the receiver is done with it.
            tokio::time::sleep(Duration::from_secs(5)).await;
        });
        let request = FileRequest {
            job: 1,
            file: 0,
            offset: 0,
        };
        let mut partial = scratch.partial(size);
        let recv = match ask(&connection, request).await.unwrap() {
            Answer::Sending(recv) => recv,
            // A reset can overtake the status byte.
            Answer::Changed => return (Ok(Received::Changed), partial, Duration::ZERO),
            Answer::Denied | Answer::Busy => panic!("the scripted sender sends"),
        };
        let (progress, _) = std::sync::mpsc::channel();
        let (_stop, mut cancel) = tokio::sync::watch::channel(false);
        let counters = TransferCounters::default();
        let started = std::time::Instant::now();
        let received = receive(recv, &mut partial, size, &progress, &counters, &mut cancel).await;
        (received, partial, started.elapsed())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_value_split_across_writes_checks() {
        let scratch = Scratch::new("split");
        let bytes = b"segment bytes".to_vec();
        let value = value_of(&bytes);
        let sent = bytes.clone();
        let (received, _) =
            receive_from(&scratch, bytes.len() as u64, move |mut send| async move {
                send.write_all(&sent).await.unwrap();
                send.write_all(&value[..7]).await.unwrap();
                tokio::time::sleep(Duration::from_millis(20)).await;
                send.write_all(&value[7..]).await.unwrap();
                send.finish().unwrap();
            })
            .await;
        assert_eq!(received.unwrap(), Received::Done { downloaded: 13 });
        assert_eq!(scratch.data(), bytes);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_wrong_value_drops_the_segment() {
        let scratch = Scratch::new("wrong");
        let (received, partial) = receive_from(&scratch, 4, |mut send| async move {
            send.write_all(b"abcd").await.unwrap();
            send.write_all(&value_of(b"abce")).await.unwrap();
            send.finish().unwrap();
        })
        .await;
        assert_eq!(received.unwrap(), Received::Mismatch);
        assert_eq!((partial.checked(), partial.written()), (0, 0));
        assert!(scratch.data().is_empty());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn bytes_past_the_end_are_malformed() {
        let scratch = Scratch::new("extra");
        let (received, _) = receive_from(&scratch, 4, |mut send| async move {
            send.write_all(b"abcd").await.unwrap();
            send.write_all(&value_of(b"abcd")).await.unwrap();
            send.write_all(b"more").await.unwrap();
            send.finish().unwrap();
        })
        .await;
        assert!(matches!(received, Err(Error::Malformed)));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_stream_that_ends_early_is_an_error() {
        let scratch = Scratch::new("short");
        let (received, partial) = receive_from(&scratch, 8, |mut send| async move {
            send.write_all(b"abcd").await.unwrap();
            send.finish().unwrap();
        })
        .await;
        assert!(matches!(received, Err(Error::Network(_))));
        assert_eq!(partial.checked(), 0);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_changed_source_is_reported() {
        let scratch = Scratch::new("changed");
        let (received, _) = receive_from(&scratch, 8, |mut send| async move {
            send.write_all(b"abcd").await.unwrap();
            send.reset(VarInt::from_u32(RESET_CHANGED)).unwrap();
        })
        .await;
        assert_eq!(received.unwrap(), Received::Changed);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_silent_sender_is_stalled() {
        let scratch = Scratch::new("stalled");
        let (received, partial, took) = receive_timed(&scratch, 8, |mut send| async move {
            send.write_all(b"abcd").await.unwrap();
            tokio::time::sleep(Duration::from_secs(4)).await;
        })
        .await;
        assert_eq!(received.unwrap(), Received::Stalled);
        assert!(took < Duration::from_secs(3), "stalled after {took:?}");
        assert_eq!(partial.checked(), 0);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn an_empty_file_needs_no_value() {
        let scratch = Scratch::new("empty");
        let (received, _) = receive_from(&scratch, 0, |mut send| async move {
            send.finish().unwrap();
        })
        .await;
        assert_eq!(received.unwrap(), Received::Done { downloaded: 0 });
    }

    /// The real sender and receiver agree, and an offset off a segment
    /// boundary is refused before anything is read.
    #[cfg(not(windows))]
    #[tokio::test(flavor = "multi_thread")]
    async fn a_served_file_arrives_whole() {
        let scratch = Scratch::new("served");
        let bytes: Vec<u8> = (0..300_000u32).map(|n| (n % 241) as u8).collect();
        let source_path = scratch.0.join("source");
        std::fs::write(&source_path, &bytes).unwrap();
        let (connection, serving, _receiver, _sender) = pair().await;
        let size = bytes.len() as u64;
        let serve_path = source_path.clone();
        tokio::spawn(async move {
            while let Ok((mut send, mut recv)) = serving.accept_bi().await {
                let request = super::super::read_request(&mut recv).await.unwrap();
                let source = super::super::Source {
                    file: std::fs::File::open(&serve_path).unwrap(),
                    size,
                    mtime_ms: 0,
                };
                let counters = TransferCounters::default();
                super::super::send(
                    &mut send,
                    source,
                    request.offset,
                    &counters,
                    || true,
                    |_, _| {},
                )
                .await;
            }
        });
        let unaligned = FileRequest {
            job: 1,
            file: 0,
            offset: 5,
        };
        assert!(matches!(
            ask(&connection, unaligned).await.unwrap(),
            Answer::Denied
        ));
        let request = FileRequest {
            job: 1,
            file: 0,
            offset: 0,
        };
        let Answer::Sending(recv) = ask(&connection, request).await.unwrap() else {
            panic!("the sender sends");
        };
        let mut partial = scratch.partial(size);
        let (progress, arrived) = std::sync::mpsc::channel();
        let (_stop, mut cancel) = tokio::sync::watch::channel(false);
        let counters = TransferCounters::default();
        let received = receive(recv, &mut partial, size, &progress, &counters, &mut cancel).await;
        assert_eq!(received.unwrap(), Received::Done { downloaded: size });
        let summary = counters.summary();
        assert_eq!(summary.bytes_received, size);
        assert!(summary.segments_checked >= 1);
        assert_eq!(scratch.data(), bytes);
        assert_eq!(arrived.try_iter().last(), Some(size));
        let _ = STATUS_DENIED;
    }

    /// Randomized properties of the stream parser, seeded so a failure
    /// replays: however a sender's stream is chunked it lands whole, and
    /// a corrupted, cut or overlong stream never lands as a file.
    mod properties {
        use rand::rngs::StdRng;
        use rand::{RngExt, SeedableRng};

        use super::*;

        /// Small segments, so streams of several stay cheap.
        const SEGMENT_LEN: u64 = 64 * 1024;

        fn stream_of(bytes: &[u8]) -> Vec<u8> {
            super::super::stream_of(bytes, SEGMENT_LEN)
        }

        /// Feeds `stream` to a fresh partial in random chunks. Ok(true) when
        /// the file landed whole, Ok(false) when a segment failed its check.
        fn land(
            scratch: &Scratch,
            size: u64,
            stream: &[u8],
            rng: &mut StdRng,
        ) -> Result<bool, Error> {
            let _ = std::fs::remove_file(scratch.0.join("data"));
            let _ = std::fs::remove_file(scratch.0.join("journal"));
            let mut partial = Partial::open_in(
                &scratch.0.join("data"),
                &scratch.0.join("journal"),
                size,
                SEGMENT_LEN,
            )
            .unwrap();
            let mut incoming = Incoming::default();
            let (progress, _) = std::sync::mpsc::channel();
            let counters = TransferCounters::default();
            let mut rest = stream;
            while !rest.is_empty() {
                let take = rng.random_range(1..=rest.len().min(3 * VALUE_LEN + 7));
                if !incoming.feed(&mut partial, &rest[..take], &progress, &counters)? {
                    return Ok(false);
                }
                rest = &rest[take..];
            }
            incoming.finish(&partial, size)?;
            Ok(true)
        }

        fn file(rng: &mut StdRng) -> Vec<u8> {
            let size = rng.random_range(0..=3 * SEGMENT_LEN as usize + 100);
            (0..size).map(|_| rng.random::<u8>()).collect()
        }

        #[test]
        fn any_chunking_lands_the_file_whole() {
            let scratch = Scratch::new("chunked");
            let mut rng = StdRng::seed_from_u64(0x5e9);
            for _ in 0..64 {
                let bytes = file(&mut rng);
                let stream = stream_of(&bytes);
                assert!(land(&scratch, bytes.len() as u64, &stream, &mut rng).unwrap());
                if !bytes.is_empty() {
                    assert_eq!(scratch.data(), bytes);
                }
            }
        }

        #[test]
        fn a_changed_byte_never_lands() {
            let scratch = Scratch::new("flipped");
            let mut rng = StdRng::seed_from_u64(0xf11b);
            for _ in 0..128 {
                let bytes = file(&mut rng);
                if bytes.is_empty() {
                    continue;
                }
                let mut stream = stream_of(&bytes);
                let at = rng.random_range(0..stream.len());
                stream[at] ^= 1 << rng.random_range(0..8);
                let landed = land(&scratch, bytes.len() as u64, &stream, &mut rng);
                assert!(!matches!(landed, Ok(true)), "byte {at} changed and landed");
            }
        }

        #[test]
        fn a_cut_stream_never_lands() {
            let scratch = Scratch::new("cut");
            let mut rng = StdRng::seed_from_u64(0xc07);
            for _ in 0..128 {
                let bytes = file(&mut rng);
                let stream = stream_of(&bytes);
                if stream.is_empty() {
                    continue;
                }
                let cut = rng.random_range(0..stream.len());
                let landed = land(&scratch, bytes.len() as u64, &stream[..cut], &mut rng);
                assert!(landed.is_err(), "cut at {cut} of {} landed", stream.len());
            }
        }

        #[test]
        fn bytes_after_the_last_value_never_land() {
            let scratch = Scratch::new("long");
            let mut rng = StdRng::seed_from_u64(0x10e);
            for _ in 0..64 {
                let bytes = file(&mut rng);
                let mut stream = stream_of(&bytes);
                let extra = rng.random_range(1..=VALUE_LEN + 3);
                stream.extend((0..extra).map(|_| rng.random::<u8>()));
                let landed = land(&scratch, bytes.len() as u64, &stream, &mut rng);
                assert!(matches!(landed, Err(Error::Malformed)));
            }
        }
    }
}
