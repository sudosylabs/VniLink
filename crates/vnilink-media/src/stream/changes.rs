//! What a consumer's pictures say changed: the regions a stream picture
//! carries, and how much of the screen a change covers.
// trace: records every ignored failure

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use vnilink::{Core, CursorState};

use crate::frame::RawFrame;
use crate::pump::FrameSink;

pub(super) struct TargetSink<'a> {
    pub(super) core: &'a Core,
    pub(super) service: bool,
    pub(super) generation: u64,
    pub(super) capture_display: u64,
    pub(super) capture_revision: Arc<AtomicU64>,
    /// When the picture being encoded was captured.
    pub(super) captured_at: Option<Instant>,
    /// The picture being encoded shows the same screen as the one before.
    pub(super) unchanged: bool,
    /// Where the screen changed since the last picture sent, in capture
    /// pixels; `None` when that is not known, as after pictures missed.
    pub(super) pending: Option<Vec<vnilink::Region>>,
    /// The capture's size and the encoded picture's, to place the regions.
    pub(super) capture: (u16, u16),
    pub(super) picture: (u16, u16),
}

impl TargetSink<'_> {
    /// Adds where a picture changed the screen to what the next picture
    /// sent says; `None` says nothing more can be said.
    pub(super) fn add(&mut self, regions: Option<&[vnilink::Region]>) {
        self.pending = self
            .pending
            .take()
            .zip(regions)
            .and_then(|(mut pending, regions)| {
                pending.extend_from_slice(regions);
                (pending.len() <= vnilink::MAX_REGIONS).then_some(pending)
            });
    }

    /// `region` of the capture in the encoded picture's pixels, grown to
    /// whole pixels.
    fn placed(&self, region: vnilink::Region) -> vnilink::Region {
        let along = |at: u16, from: u16, to: u16, up: bool| -> u16 {
            let scaled = u32::from(at) * u32::from(to);
            let from = u32::from(from.max(1));
            (if up {
                scaled.div_ceil(from)
            } else {
                scaled / from
            })
            .min(u32::from(to)) as u16
        };
        let ((cw, ch), (pw, ph)) = (self.capture, self.picture);
        let left = along(region.x, cw, pw, false);
        let top = along(region.y, ch, ph, false);
        let right = along(region.x.saturating_add(region.width), cw, pw, true);
        let bottom = along(region.y.saturating_add(region.height), ch, ph, true);
        vnilink::Region {
            x: left,
            y: top,
            width: right - left,
            height: bottom - top,
        }
    }
}

impl FrameSink for TargetSink<'_> {
    fn publish_frame(
        &mut self,
        id: u64,
        mut frame: vnilink::EncodedFrame,
    ) -> Result<(), vnilink::Error> {
        // Attest the actual capture source: Core backstops any remaining
        // zero from the owner's accepted selection, but only the producer
        // knows which display an unselected ("follows capture") target
        // actually receives.
        if frame.display_id == 0 {
            frame.display_id = self.capture_display;
        }
        if frame.timing.captured_at.is_none() {
            frame.timing.captured_at = self.captured_at;
        }
        // A stream picture says where it differs from the one sent before
        // it: the screen's changes since, pictures the encoder skipped
        // included. One the encoder made of an unchanged screen after such
        // changes is a change too.
        if frame.role != vnilink::FrameRole::Still {
            let regions = self.pending.replace(Vec::new());
            let changed = regions.as_ref().is_none_or(|regions| !regions.is_empty());
            if self.unchanged && !changed {
                frame = frame.unchanged();
            } else {
                frame = frame.with_regions(regions.map(|regions| {
                    regions
                        .into_iter()
                        .map(|region| self.placed(region))
                        .collect()
                }));
            }
        }
        // Attest the revision the pixels were captured for, read live:
        // a supervisor re-attesting an unchanged capture updates the
        // stamp without restarting this worker. A capture that does not
        // know its revision (0) falls back to the current topology; a
        // pinned capture keeps its attestation even after the topology
        // advanced, so a stale crop is never relabeled current.
        if frame.revision == 0 {
            let pinned = self.capture_revision.load(Ordering::Acquire);
            frame.revision = if pinned != 0 {
                pinned
            } else {
                self.core
                    .topology()
                    .map(|topology| topology.revision)
                    .unwrap_or(0)
            };
        }
        tracing::trace!(
            target: vnilink::trace::STREAM,
            service = id,
            role = ?frame.role,
            keyframe = frame.keyframe,
            bytes = frame.payload.len(),
            timestamp_us = frame.timestamp_us,
            regions = frame.regions.as_ref().map(Vec::len),
            "picture sent"
        );
        if self.service {
            self.core.publish_desktop_video(id, self.generation, frame)
        } else {
            self.core.publish_video(id, frame)
        }
    }
}

/// Where `cursor` is drawn on the screen, when it is.
pub(super) fn pointer_bounds(cursor: &CursorState) -> Option<vnilink::Region> {
    let image = cursor.image.as_ref().filter(|_| cursor.visible)?;
    let left = i64::from(cursor.position.0) - i64::from(image.hotspot().0);
    let top = i64::from(cursor.position.1) - i64::from(image.hotspot().1);
    let right = (left + i64::from(image.width())).clamp(0, i64::from(u16::MAX));
    let bottom = (top + i64::from(image.height())).clamp(0, i64::from(u16::MAX));
    let (left, top) = (left.clamp(0, right), top.clamp(0, bottom));
    Some(vnilink::Region {
        x: left as u16,
        y: top as u16,
        width: (right - left) as u16,
        height: (bottom - top) as u16,
    })
}

/// The most of the screen a change may cover and leave a still where it
/// is: a clock, a caret or a few typed letters. The viewer redraws such a
/// change from the stream over the still, tile by tile.
const SMALL_CHANGE: f32 = 0.02;

/// Whether `frame` changed only a little of the screen, as its source says;
/// a source that cannot say changed all of it.
pub(super) fn small_change(frame: &RawFrame) -> bool {
    frame
        .changed()
        .is_some_and(|changes| changes.share <= SMALL_CHANGE)
}
