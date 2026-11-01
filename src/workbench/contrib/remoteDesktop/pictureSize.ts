// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import * as nls from '../../../nls.js';

/**
 * How the picture is sized in its tab: fitted, never larger than its own
 * pixels; stretched to fill the tab; or zoomed, where 1 puts each pixel of
 * the picture on one pixel of this screen.
 */
export type PictureSize = 'fit' | 'stretch' | number;

/** The zooms offered beside fitting and stretching; 1 is the original size. */
export const zooms: readonly number[] = [0.5, 0.75, 1, 1.5, 2];

export function pictureSizeFrom(value: unknown): PictureSize {
	if (value === 'fit' || value === 'stretch') {
		return value;
	}
	if (value === 'original') {
		return 1;
	}
	const zoom = Number(value);
	return zooms.includes(zoom) ? zoom : 'fit';
}

interface Size {
	readonly width: number;
	readonly height: number;
}

/**
 * The picture's size on screen in CSS pixels, for a picture of `width` ×
 * `height` pixels in a stage of `stage` CSS pixels on a screen of `ratio`
 * device pixels per CSS pixel. Whole device pixels, so a picture at its
 * original size is drawn without resampling.
 */
export function shownSize(size: PictureSize, width: number, height: number, stage: Size, ratio: number): Size {
	const fill = Math.min(stage.width * ratio / width, stage.height * ratio / height);
	const room = Number.isFinite(fill) && fill > 0 ? fill : 1;
	const scale = size === 'fit' ? Math.min(1, room) : size === 'stretch' ? room : size;
	return { width: Math.max(1, Math.floor(width * scale)) / ratio, height: Math.max(1, Math.floor(height * scale)) / ratio };
}

export function pictureSizeLabel(size: PictureSize): string {
	switch (size) {
		case 'fit': return nls.localize('desktop.pictureFit', 'Fit');
		case 'stretch': return nls.localize('desktop.pictureStretch', 'Stretch');
		default:
			return size === 1 ? nls.localize('desktop.pictureOriginal', 'Original size') : nls.localize('desktop.pictureZoom', '{0}%', Math.round(size * 100));
	}
}
