// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import type { Event } from '../../../../base/common/event.js';
import { Disposable } from '../../../../base/common/lifecycle.js';
import type { DataChannelRequest, DataPacket, DesktopAudioStatus, DesktopCursor, DesktopMediaStatus } from '../../common/dataChannel.js';
import type { IDataChannel, IDataChannelService } from '../../common/dataChannelService.js';
import type { DesktopInputCommand } from '../../common/engine.js';
import type { FileAnswer, FileOperation } from '../../common/files.js';

export type TestDataChannelRequest = Omit<DataChannelRequest, 'requestId'>;

/**
 * One channel a {@link TestDataChannelService} opened. A test delivers
 * packets through `consume` and failures through `onError`, as the engine
 * host would; what the subject sent is kept in `inputs`, `writes`,
 * `resizes`, `fileOperations` and `closed`.
 */
export class TestDataChannel implements IDataChannel {
	/** The other computer's pointer never moves on a test channel. */
	public readonly onCursor: Event<DesktopCursor> = () => Disposable.None;
	public readonly inputs: DesktopInputCommand[] = [];
	public readonly writes: Uint8Array[] = [];
	public readonly resizes: [rows: number, columns: number][] = [];
	public readonly fileOperations: FileOperation[] = [];
	public keyframeRequests = 0;
	public closed = false;

	constructor(
		public readonly request: TestDataChannelRequest,
		public readonly consume: (packet: DataPacket) => void | Promise<void>,
		public readonly onError: (error: Error) => void,
		private readonly answer: (operation: FileOperation) => FileAnswer | Promise<FileAnswer>,
		private readonly onClose: (channel: TestDataChannel) => void,
		private readonly written: (data: Uint8Array) => Promise<void> = () => Promise.resolve(),
	) { }

	public input(command: DesktopInputCommand): void {
		this.inputs.push(command);
	}

	public write(data: Uint8Array): Promise<void> {
		this.writes.push(data);
		return this.written(data);
	}

	public async fileOperation(operation: FileOperation): Promise<FileAnswer> {
		this.fileOperations.push(operation);
		return this.answer(operation);
	}

	public desktopAudio(): Promise<DesktopAudioStatus> {
		return Promise.reject(new Error('This test channel carries no sound'));
	}

	public mediaStatus(): Promise<DesktopMediaStatus | undefined> {
		return Promise.resolve(undefined);
	}

	public requestKeyframe(): void {
		this.keyframeRequests++;
	}

	public resize(rows: number, columns: number): void {
		this.resizes.push([rows, columns]);
	}

	public close(): void {
		this.closed = true;
		this.onClose(this);
	}
}

/**
 * An {@link IDataChannelService} whose channels a test scripts: `refuse`
 * names the error a request is refused with (none opens it),
 * `fileAnswer` answers each file operation and `writeAnswer` settles each
 * write. Every request it was asked is
 * kept in `requests`, every channel it opened in `opened`, and every
 * channel's close in `closes`, in order.
 */
export class TestDataChannelService implements IDataChannelService {
	declare public readonly _serviceBrand: undefined;
	public readonly requests: TestDataChannelRequest[] = [];
	public readonly opened: TestDataChannel[] = [];
	public readonly closes: TestDataChannelRequest[] = [];
	public refuse: (request: TestDataChannelRequest) => Error | undefined = () => undefined;
	public fileAnswer: (operation: FileOperation) => FileAnswer | Promise<FileAnswer> = () => ({ ok: null });
	public writeAnswer: (data: Uint8Array) => Promise<void> = () => Promise.resolve();

	public open(
		request: TestDataChannelRequest,
		consume: (packet: DataPacket) => void | Promise<void>,
		onError: (error: Error) => void,
	): Promise<IDataChannel> {
		this.requests.push(request);
		const refusal = this.refuse(request);
		if (refusal) {
			return Promise.reject(refusal);
		}
		const channel = new TestDataChannel(
			request, consume, onError,
			operation => this.fileAnswer(operation),
			closed => this.closes.push(closed.request),
			data => this.writeAnswer(data),
		);
		this.opened.push(channel);
		return Promise.resolve(channel);
	}

	/** The channels opened for `kind`, oldest first. */
	public openedFor(kind: TestDataChannelRequest['kind']): TestDataChannel[] {
		return this.opened.filter(channel => channel.request.kind === kind);
	}
}
