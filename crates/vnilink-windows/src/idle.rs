//! Damage-driven Desktop Duplication keeps one picture, never an input backlog.
use vnilink_media::{Error, RawFrame};

pub(crate) fn picture(
    result: Result<Option<RawFrame>, Error>,
    last: &mut Option<RawFrame>,
    timestamp_us: u64,
) -> Result<Option<RawFrame>, Error> {
    match result {
        Ok(Some(frame)) => *last = Some(frame),
        Ok(None) => {}
        Err(error) => {
            *last = None;
            return Err(error);
        }
    }
    Ok(last.clone().map(|frame| frame.with_timestamp(timestamp_us)))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn idle_picture_is_retained_with_fresh_time_but_native_failure_clears_it() {
        let mut last = None;
        assert!(picture(Ok(None), &mut last, 1).unwrap().is_none());
        let first = RawFrame::bgra(2, 2, 2, vec![19; 16]).unwrap();
        let delivered = picture(Ok(Some(first)), &mut last, 3).unwrap().unwrap();
        assert_eq!(delivered.timestamp_us(), 3);
        let idle = picture(Ok(None), &mut last, 4).unwrap().unwrap();
        assert_eq!(idle.pixels(), delivered.pixels());
        assert_eq!(idle.timestamp_us(), 4);
        let changed = RawFrame::bgra(2, 2, 5, vec![47; 16]).unwrap();
        assert_eq!(
            picture(Ok(Some(changed)), &mut last, 6)
                .unwrap()
                .unwrap()
                .pixels(),
            &[47; 16]
        );
        assert!(matches!(
            picture(Err(Error::CaptureRejected("access lost")), &mut last, 7),
            Err(Error::CaptureRejected("access lost"))
        ));
        assert!(picture(Ok(None), &mut last, 8).unwrap().is_none());
    }
}
