// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! One physical capture, one encoder and queue per consumer.
//!
//! The reader is the only caller of
//! [`FrameSource::poll_frame`](crate::FrameSource::poll_frame). Each
//! consumer keeps the newest raw picture and encodes it on its own thread at
//! its own quality. A slow encoder skips intervening pictures; it does not
//! queue them and it does not stall the reader or any sibling.
//!
//! A source that can scale as it captures
//! ([`FrameSource::deliver_at`](crate::FrameSource::deliver_at)) is asked
//! for the largest size any consumer encodes, so that consumer's encoder
//! takes the picture as it comes; smaller consumers scale it down.

mod capture;
mod changes;
mod stills;
mod target;

pub use target::serve_shared_capture;
