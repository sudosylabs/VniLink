// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

use std::os::fd::FromRawFd;

use super::*;

#[test]
fn a_linear_dma_buf_imports_as_opaque_red() {
    let mut importer = Importer::new();
    let _ = importer.modifiers();
    if importer.devices.is_empty() {
        return;
    }
    let device = importer.devices[0].ptr;
    unsafe {
        let bo = gbm_bo_create(
            device,
            64,
            64,
            DRM_FORMAT_ARGB8888,
            GBM_BO_USE_LINEAR | GBM_BO_USE_RENDERING,
        );
        if bo.is_null() {
            return;
        }
        let mut stride = 0u32;
        let mut map_data = ptr::null_mut();
        let mapped = gbm_bo_map(
            bo,
            0,
            0,
            64,
            64,
            GBM_BO_TRANSFER_READ | GBM_BO_TRANSFER_WRITE,
            &mut stride,
            &mut map_data,
        );
        if mapped.is_null() || stride < 64 * 4 {
            gbm_bo_destroy(bo);
            return;
        }
        for y in 0..64usize {
            let row = std::slice::from_raw_parts_mut(
                mapped.cast::<u8>().add(y * stride as usize),
                64 * 4,
            );
            for pixel in row.chunks_exact_mut(4) {
                pixel.copy_from_slice(&[0, 0, 255, 255]);
            }
        }
        gbm_bo_unmap(bo, map_data);
        let fd = gbm_bo_get_fd(bo);
        let modifier = gbm_bo_get_modifier(bo);
        if fd < 0 {
            gbm_bo_destroy(bo);
            return;
        }
        let _kept = OwnedFd::from_raw_fd(fd);
        let copied = importer.map(
            64,
            64,
            DRM_FORMAT_ARGB8888,
            modifier,
            modifier != DRM_FORMAT_MOD_INVALID,
            &[Plane {
                fd,
                stride: stride as c_int,
                offset: 0,
            }],
        );
        gbm_bo_destroy(bo);
        let (bytes, row) = copied.expect("DMA-BUF import failed");
        let frame =
            crate::pixels::raw_frame_from_bgra(64, 64, row, 0, &bytes).expect("imported frame");
        assert_eq!(&frame.row(32)[32 * 4..32 * 4 + 4], &[0, 0, 255, 255]);
    }
}
