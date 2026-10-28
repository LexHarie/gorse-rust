// Ports: github.com/juju/ratelimit reader.go @ v1.0.2
//
// Copyright 2014 Canonical Ltd.
// Licensed under the LGPLv3 with static-linking exception.
// See LICENSE file for details.
//
// Modified 2026-10-19 for gorse-rust: translated from Go to Rust.

use std::io;
use std::sync::Arc;

use crate::Bucket;

/// Reader returns a reader that is rate limited by
/// the given token bucket. Each token in the bucket
/// represents one byte.
#[derive(Debug)]
pub struct Reader<R> {
    r: R,
    bucket: Arc<Bucket>,
}

impl<R> Reader<R> {
    /// Go's `ratelimit.Reader(r, bucket)`.
    pub fn new(r: R, bucket: Arc<Bucket>) -> Reader<R> {
        Reader { r, bucket }
    }
}

impl<R: io::Read> io::Read for Reader<R> {
    /// Reads, then waits for a token per byte read. Go's reader can return
    /// bytes and an error together and waits for those bytes; a Rust read
    /// returns one or the other, and an error waits for nothing.
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.r.read(buf)?;
        if n == 0 {
            return Ok(n);
        }
        self.bucket.wait(i64::try_from(n).unwrap_or(i64::MAX));
        Ok(n)
    }
}

/// Writer returns a reader that is rate limited by
/// the given token bucket. Each token in the bucket
/// represents one byte.
#[derive(Debug)]
pub struct Writer<W> {
    w: W,
    bucket: Arc<Bucket>,
}

impl<W> Writer<W> {
    /// Go's `ratelimit.Writer(w, bucket)`.
    pub fn new(w: W, bucket: Arc<Bucket>) -> Writer<W> {
        Writer { w, bucket }
    }
}

impl<W: io::Write> io::Write for Writer<W> {
    /// Waits for a token per byte of `buf`, then writes.
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.bucket
            .wait(i64::try_from(buf.len()).unwrap_or(i64::MAX));
        self.w.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.w.flush()
    }
}

#[cfg(test)]
mod tests {
    //! juju has no tests for reader.go; these check the token accounting.

    use super::*;
    use crate::Clock;
    use std::io::{Read, Write};
    use std::sync::Mutex;
    use std::time::Instant;

    /// Records the sleeps instead of sleeping.
    struct Fake {
        now: Instant,
        slept: Mutex<Vec<i64>>,
    }

    impl Clock for Fake {
        fn now(&self) -> Instant {
            self.now
        }
        fn sleep(&self, d: i64) {
            self.slept.lock().unwrap_or_else(|e| e.into_inner()).push(d);
        }
    }

    fn bucket() -> (Arc<Fake>, Arc<Bucket>) {
        let clock = Arc::new(Fake {
            now: Instant::now(),
            slept: Mutex::new(Vec::new()),
        });
        // A token every millisecond, 4 to start with.
        let bucket = Bucket::with_clock(1_000_000, 4, Some(clock.clone()));
        (clock, Arc::new(bucket))
    }

    #[test]
    fn reader_waits_for_the_bytes_read() {
        let (clock, bucket) = bucket();
        let mut r = Reader::new(&b"0123456789"[..], Arc::clone(&bucket));
        let mut buf = [0; 6];
        assert_eq!(r.read(&mut buf).ok(), Some(6));
        assert_eq!(r.read(&mut buf).ok(), Some(4));
        assert_eq!(r.read(&mut buf).ok(), Some(0));
        // 4 free tokens, then 2 and 4 owed: ticks 2 and 6.
        assert_eq!(*clock.slept.lock().unwrap_or_else(|e| e.into_inner()), [
            2_000_000, 6_000_000
        ]);
        assert_eq!(bucket.available(), -6);
    }

    #[test]
    fn writer_waits_for_the_whole_buffer() {
        let (clock, bucket) = bucket();
        let mut out = Vec::new();
        let mut w = Writer::new(&mut out, Arc::clone(&bucket));
        assert_eq!(w.write(b"abc").ok(), Some(3));
        assert_eq!(w.write(b"defg").ok(), Some(4));
        assert!(w.flush().is_ok());
        assert_eq!(out, b"abcdefg");
        assert_eq!(*clock.slept.lock().unwrap_or_else(|e| e.into_inner()), [
            3_000_000
        ]);
    }
}
