// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

use vnilink_media::Error;

use crate::encode::MacEncoder;

#[test]
fn an_odd_encoder_size_is_rejected() {
    assert!(matches!(
        MacEncoder::open(63, 64, 30, 500_000, false, vnilink::YuvMatrix::Bt709),
        Err(Error::InvalidFrame("H.264 width and height must be even"))
    ));
}
