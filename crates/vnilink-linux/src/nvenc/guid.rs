// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! The `static const GUID`s of `nvEncodeAPI.h` this encoder uses, copied
//! from the header the bindings were generated from.

use super::bindings::GUID;

pub(super) const CODEC_H264: GUID = GUID {
    Data1: 0x6bc82762,
    Data2: 0x4e63,
    Data3: 0x4ca4,
    Data4: [0xaa, 0x85, 0x1e, 0x50, 0xf3, 0x21, 0xf6, 0xbf],
};
pub(super) const CODEC_HEVC: GUID = GUID {
    Data1: 0x790cdc88,
    Data2: 0x4522,
    Data3: 0x4d7b,
    Data4: [0x94, 0x25, 0xbd, 0xa9, 0x97, 0x5f, 0x76, 0x3],
};
pub(super) const CODEC_AV1: GUID = GUID {
    Data1: 0x0a352289,
    Data2: 0x0aa7,
    Data3: 0x4759,
    Data4: [0x86, 0x2d, 0x5d, 0x15, 0xcd, 0x16, 0xd2, 0x54],
};
pub(super) const H264_PROFILE_BASELINE: GUID = GUID {
    Data1: 0x727bcaa,
    Data2: 0x78c4,
    Data3: 0x4c83,
    Data4: [0x8c, 0x2f, 0xef, 0x3d, 0xff, 0x26, 0x7c, 0x6a],
};
pub(super) const H264_PROFILE_HIGH: GUID = GUID {
    Data1: 0xe7cbc309,
    Data2: 0x4f7a,
    Data3: 0x4b89,
    Data4: [0xaf, 0x2a, 0xd5, 0x37, 0xc9, 0x2b, 0xe3, 0x10],
};
pub(super) const HEVC_PROFILE_MAIN: GUID = GUID {
    Data1: 0xb514c39a,
    Data2: 0xb55b,
    Data3: 0x40fa,
    Data4: [0x87, 0x8f, 0xf1, 0x25, 0x3b, 0x4d, 0xfd, 0xec],
};
pub(super) const AV1_PROFILE_MAIN: GUID = GUID {
    Data1: 0x5f2a39f5,
    Data2: 0xf14e,
    Data3: 0x4f95,
    Data4: [0x9a, 0x9e, 0xb7, 0x6d, 0x56, 0x8f, 0xcf, 0x97],
};
pub(super) const PRESET_P3: GUID = GUID {
    Data1: 0x36850110,
    Data2: 0x3a07,
    Data3: 0x441f,
    Data4: [0x94, 0xd5, 0x36, 0x70, 0x63, 0x1f, 0x91, 0xf6],
};
