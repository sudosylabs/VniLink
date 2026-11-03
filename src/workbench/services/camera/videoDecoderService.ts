// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import { createDecorator } from '../../../platform/instantiation/common/instantiation.js';
import type { StreamMatrix } from './decoderColour.js';
import type { CameraDecoder } from './engineCameraStream.js';

export const IVideoDecoderService = createDecorator<IVideoDecoderService>('videoDecoderService');

/** The window's video decoders, for camera and screen pictures. */
export interface IVideoDecoderService {
	readonly _serviceBrand: undefined;

	/**
	 * A decoder for `codec`, hardware first, or `undefined` when this window
	 * can't decode it. `output` receives each picture and `error` reports a
	 * decoder that failed after it was created.
	 */
	createDecoder(codec: string, output: (frame: VideoFrame) => void, error: () => void): Promise<CameraDecoder | undefined>;

	/** The matrix this window decodes `config` in, or `undefined` where it does not decode with it. */
	support(config: VideoDecoderConfig): Promise<StreamMatrix | undefined>;
}
