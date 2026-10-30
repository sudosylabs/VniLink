// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! Ordered partial writes with cancellation between each native write.
use std::{
    io::Write,
    sync::atomic::{AtomicBool, Ordering},
};
pub(crate) fn write_input(
    writer: &mut impl Write,
    bytes: &[u8],
    stop: &AtomicBool,
) -> std::io::Result<()> {
    let mut offset = 0;
    while offset < bytes.len() {
        if stop.load(Ordering::Acquire) {
            return Err(std::io::ErrorKind::Interrupted.into());
        }
        match writer.write(&bytes[offset..]) {
            Ok(0) => return Err(std::io::ErrorKind::WriteZero.into()),
            Ok(count) => offset += count,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Partial {
        bytes: Vec<u8>,
        calls: usize,
        stop: Option<std::sync::Arc<AtomicBool>>,
    }
    impl Write for Partial {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.calls += 1;
            if self.calls == 2 {
                if let Some(stop) = &self.stop {
                    stop.store(true, Ordering::Release);
                }
                return Err(std::io::ErrorKind::Interrupted.into());
            }
            let count = bytes.len().min(2);
            self.bytes.extend(&bytes[..count]);
            Ok(count)
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    #[test]
    fn partial_writes_preserve_order_and_retry_interruptions() {
        let mut writer = Partial {
            bytes: vec![],
            calls: 0,
            stop: None,
        };
        write_input(&mut writer, b"abcdef", &AtomicBool::new(false)).unwrap();
        assert_eq!(writer.bytes, b"abcdef");
        assert_eq!(writer.calls, 4);
    }
    #[test]
    fn cancelled_interrupted_write_never_resubmits_remaining_input() {
        let stop = std::sync::Arc::new(AtomicBool::new(false));
        let mut writer = Partial {
            bytes: vec![],
            calls: 0,
            stop: Some(stop.clone()),
        };
        assert_eq!(
            write_input(&mut writer, b"abcdef", &stop)
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::Interrupted
        );
        assert_eq!(writer.bytes, b"ab");
        assert_eq!(writer.calls, 2);
    }
    #[test]
    fn already_cancelled_write_does_not_enter_native_io() {
        let mut writer = Partial {
            bytes: vec![],
            calls: 0,
            stop: None,
        };
        assert!(write_input(&mut writer, b"abc", &AtomicBool::new(true)).is_err());
        assert_eq!(writer.calls, 0);
    }
}
