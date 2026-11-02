// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import type { Event } from '../../../base/common/event.js';
import { createDecorator } from '../../instantiation/common/instantiation.js';
import type { DataChannelRequest, DataPacket, DesktopAudioOperation, DesktopAudioStatus, DesktopCursor, DesktopDecoderReport, DesktopMediaStatus } from './dataChannel.js';
import type { DesktopInputCommand } from './engine.js';
import type { FileAnswer, FileOperation } from './files.js';

/**
 * The window's end of one native data channel. Each kind of channel uses
 * the part that carries its traffic; a call that does not belong to the
 * channel's kind is refused by the engine host.
 */
export interface IDataChannel {
	/** The other computer's pointer moved or changed shape, on a desktop picture channel. */
	readonly onCursor: Event<DesktopCursor>;
	input(command: DesktopInputCommand): void;
	write(data: Uint8Array): Promise<void>;
	fileOperation(operation: FileOperation): Promise<FileAnswer>;
	desktopAudio(operation: DesktopAudioOperation): Promise<DesktopAudioStatus>;
	mediaStatus(decoder: DesktopDecoderReport): Promise<DesktopMediaStatus | undefined>;
	requestKeyframe(): void;
	resize(rows: number, columns: number): void;
	close(): void;
}

export const IDataChannelService = createDecorator<IDataChannelService>('dataChannelService');

/** Opens the engine's data channels for this window. */
export interface IDataChannelService {
	readonly _serviceBrand: undefined;

	/**
	 * Opens a channel for `request`. Packets reach `consume` in order, the
	 * next once the last one's promise settles; `onError` reports a channel
	 * that failed or stopped answering after it opened. A refusal rejects
	 * with the engine's error category (`Busy: …`, `Unauthorized: …`).
	 */
	open(
		request: Omit<DataChannelRequest, 'requestId'>,
		consume: (packet: DataPacket) => void | Promise<void>,
		onError: (error: Error) => void,
	): Promise<IDataChannel>;
}
