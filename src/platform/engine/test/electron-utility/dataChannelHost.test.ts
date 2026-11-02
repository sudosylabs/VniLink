// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'assert';
import { DeferredPromise, timeout } from '../../../../base/common/async.js';
import { ensureNoDisposablesAreLeakedInTestSuite } from '../../../../base/test/common/utils.js';
import { runWithFakedTimers } from '../../../../base/test/common/virtualScheduling/index.js';
import type { DataChannelRequest, DesktopCursor } from '../../common/dataChannel.js';
import type { DesktopInputCommand } from '../../common/engine.js';
import { DataChannelHost, type NativeDataChannel } from '../../electron-utility/dataChannelHost.js';

type NativePacket = NonNullable<ReturnType<NativeDataChannel['read']>>;

type NativeCursor = NonNullable<ReturnType<NativeDataChannel['cursor']>>;

/** A message the host posted to the window. */
interface Posted {
	readonly type: string;
	readonly [field: string]: unknown;
}

/**
 * The engine's end of a channel: `read` and `cursor` hand out what a test
 * queued, and every input and close is counted.
 */
class TestNativeDataChannel implements NativeDataChannel {
	public readonly packets: NativePacket[] = [];
	public readonly cursors: NativeCursor[] = [];
	public readonly inputs: DesktopInputCommand[] = [];
	public readonly writes: Uint8Array[] = [];
	public readonly resizes: [number, number][] = [];
	public closed = 0;
	public reads = 0;
	public keyframes = 0;
	public onReady?: (callback: () => void) => void;
	public desktopAudio: (operation: string) => string = () => {
		throw new Error('This channel plays no sound');
	};

	public read(): NativePacket | null {
		this.reads++;
		return this.packets.shift() ?? null;
	}

	public cursor(): NativeCursor | null {
		return this.cursors.shift() ?? null;
	}

	public input(command: string): void {
		this.inputs.push(JSON.parse(command) as DesktopInputCommand);
	}

	public close(): void {
		this.closed++;
	}

	public requestKeyframe(): void {
		this.keyframes++;
	}

	public mediaStatus(): string {
		return 'null';
	}

	public write(data: Uint8Array): void {
		this.writes.push(data);
	}

	public resize(rows: number, columns: number): void {
		this.resizes.push([rows, columns]);
	}

	public fileOperation: (command: string) => Promise<string> = () => Promise.resolve('{}');
}

const request: DataChannelRequest = { kind: 'video', requestId: 'test', sessionId: '1', generation: '2' };

suite('DataChannelHost', () => {
	ensureNoDisposablesAreLeakedInTestSuite();
	const hosts: DataChannelHost[] = [];

	/** A host for a `kind` channel whose window end records what it was posted, with `receive` sending it a message. */
	function channel(kind: DataChannelRequest['kind'] = 'video') {
		const messages: Posted[] = [];
		let listener: (event: { data: unknown }) => void = () => {
			throw new Error('The host listens to no port');
		};
		const port = {
			on: (name: 'message' | 'close', fn: (event: { data: unknown }) => void) => {
				if (name === 'message') {
					listener = fn;
				}
			},
			start: () => undefined,
			close: () => undefined,
			postMessage: (message: unknown) => messages.push(message as Posted),
		};
		const host = new DataChannelHost(port, { ...request, kind }, () => undefined);
		hosts.push(host);
		const native = new TestNativeDataChannel();
		return {
			host, native, messages,
			receive: (data: unknown) => {
				listener({ data });
			},
			sent: () => messages.filter(message => message.type === 'data'),
			cursors: () => messages.filter(message => message.type === 'cursor').map(message => message.cursor as DesktopCursor),
		};
	}
	teardown(() => {
		for (const host of hosts.splice(0)) {
			host.close();
		}
	});
	const videoPackets = (count: number): NativePacket[] => Array.from({ length: count }, (_, index) => ({ kind: 'video', data: new Uint8Array([index]), metadata: '{}' }));
	suite('flow control', () => {
		test('a desktop picture keeps four packets on the way, in order, with their bytes', () => {
			const { host, native, sent } = channel();
			const bytes = new Uint8Array([0, 255, 19]);
			native.packets.push({ kind: 'video', data: bytes, metadata: '{}' }, ...videoPackets(5));
			host.start(native);
			assert.deepStrictEqual(sent().map(message => message.sequence), [1, 2, 3, 4]);
			assert.deepStrictEqual(sent()[0].data, bytes);
		});
		test('an acknowledgement lets the next packet go', () => {
			const { host, native, sent, receive } = channel();
			native.packets.push(...videoPackets(6));
			host.start(native);
			receive({ type: 'ack', sequence: 1 });
			assert.deepStrictEqual(sent().map(message => message.sequence), [1, 2, 3, 4, 5]);
		});
		test('acknowledging out of order terminates the channel', () => {
			const { host, native, receive } = channel();
			native.packets.push(...videoPackets(6));
			host.start(native);
			receive({ type: 'ack', sequence: 1 });
			receive({ type: 'ack', sequence: 3 });
			assert.strictEqual(native.closed, 1);
		});
		test('other channels keep one packet on the way', () => {
			const { host, native, sent, receive } = channel('terminal');
			native.packets.push({ kind: 'data', data: new Uint8Array([1]), metadata: '{}' }, { kind: 'data', data: new Uint8Array([2]), metadata: '{}' });
			host.start(native);
			assert.strictEqual(sent().length, 1);
			receive({ type: 'ack', sequence: 1 });
			assert.strictEqual(sent().length, 2);
		});
		test('a terminal exit is consumed before the channel closes', () => {
			const { host, native, messages, receive } = channel('terminal');
			native.packets.push({ kind: 'exited', metadata: '{"code":7}' });
			host.start(native);
			assert.strictEqual(native.closed, 0);
			receive({ type: 'ack', sequence: 1 });
			assert.strictEqual(native.closed, 1);
			assert.strictEqual(messages.at(-1)?.type, 'closed');
		});
	});
	suite('waking', () => {
		test('a picture channel asks the engine to wake it', () => {
			const { host, native } = channel();
			let asked = false;
			native.onReady = () => {
				asked = true;
			};
			host.start(native);
			assert.strictEqual(asked, true);
		});
		test('a picture channel the engine wakes is not read every few milliseconds', () => runWithFakedTimers({ maxTaskCount: 1000 }, async () => {
			const { host, native } = channel();
			native.onReady = () => undefined;
			host.start(native);
			const before = native.reads;
			await timeout(1000);
			host.close();
			// Only the safety read, a few times a second, should a wake be missed.
			assert.ok(native.reads - before <= 4, `an empty channel was read ${String(native.reads - before)} times in a second`);
		}));
		test('a ready picture is sent when the engine says so', () => {
			const { host, native, sent } = channel();
			const wakes: (() => void)[] = [];
			native.onReady = callback => {
				wakes.push(callback);
			};
			host.start(native);
			native.packets.push(...videoPackets(1));
			wakes[0]();
			assert.strictEqual(sent().length, 1);
		});
	});
	suite('pointer', () => {
		test('the other pointer travels beside the picture, its image only when it changed', () => runWithFakedTimers({ maxTaskCount: 1000 }, async () => {
			const { host, native, cursors } = channel();
			native.cursors.push(
				{ x: 3, y: 4, visible: true, image: { width: 1, height: 1, hotspotX: 0, hotspotY: 0, pixels: new Uint8Array([1, 2, 3, 4]) } },
				{ x: 5, y: 6, visible: true, image: null },
			);
			host.start(native);
			await timeout(20);
			host.close();
			assert.deepStrictEqual(cursors().map(cursor => [cursor.x, cursor.y, 'image' in cursor]), [[3, 4, true], [5, 6, false]]);
		}));
		test('the pointer image crosses as plain bytes', () => runWithFakedTimers({ maxTaskCount: 1000 }, async () => {
			const { host, native, cursors } = channel();
			const pixels = Buffer.from([1, 2, 3, 4]);
			native.cursors.push({ x: 3, y: 4, visible: true, image: { width: 1, height: 1, hotspotX: 0, hotspotY: 0, pixels } });
			host.start(native);
			await timeout(8);
			host.close();
			const image = cursors()[0].image;
			assert.ok(image?.pixels instanceof Uint8Array && !Buffer.isBuffer(image.pixels));
		}));
		test('a pointer image that does not fill its size ends the channel instead of reaching the window', () => runWithFakedTimers({ maxTaskCount: 1000 }, async () => {
			const { host, native, cursors } = channel();
			native.cursors.push({ x: 1, y: 1, visible: true, image: { width: 2, height: 1, hotspotX: 0, hotspotY: 0, pixels: new Uint8Array([1]) } });
			host.start(native);
			await timeout(8);
			host.close();
			assert.strictEqual(native.closed, 1);
			assert.deepStrictEqual(cursors(), []);
		}));
		test('a terminal never asks for a pointer', () => runWithFakedTimers({ maxTaskCount: 1000 }, async () => {
			const { host, native } = channel('terminal');
			native.cursors.push({ x: 1, y: 1, visible: true });
			host.start(native);
			await timeout(1000);
			host.close();
			assert.strictEqual(native.cursors.length, 1);
		}));
	});
	suite('input', () => {
		const key = (sessionId: string) => ({ type: 'submitKey', sessionId, generation: '2', usage: 4, down: true });
		test('input for the channel\'s own session reaches the engine', () => {
			const { host, native, receive } = channel('input');
			host.start(native);
			receive({ type: 'input', id: 1, command: key('1') });
			assert.deepStrictEqual(native.inputs, [key('1')]);
		});
		test('input naming another session ends the channel without reaching the engine', () => {
			const { host, native, receive } = channel('input');
			host.start(native);
			receive({ type: 'input', id: 2, command: key('3') });
			assert.deepStrictEqual(native.inputs, []);
			assert.strictEqual(native.closed, 1);
		});
	});
	suite('desktop audio', () => {
		const status = { phase: 'unavailable', enabled: true, muted: false, playbackFailed: true, error: { code: 'Unsupported', message: 'No output' } };
		test('a desktop audio request is answered with the engine\'s status', () => {
			const { host, native, messages, receive } = channel();
			const operations: unknown[] = [];
			native.desktopAudio = operation => {
				operations.push(JSON.parse(operation));
				return JSON.stringify(status);
			};
			host.start(native);
			receive({ type: 'desktopAudio', id: 1, operation: { type: 'status' } });
			assert.deepStrictEqual(operations, [{ type: 'status' }]);
			assert.deepStrictEqual(messages.at(-1), { type: 'desktopAudio', id: 1, status });
		});
		test('unavailable audio leaves the picture running', () => runWithFakedTimers({ maxTaskCount: 1000 }, async () => {
			const { host, native, sent, receive } = channel();
			native.desktopAudio = () => JSON.stringify(status);
			host.start(native);
			receive({ type: 'desktopAudio', id: 1, operation: { type: 'status' } });
			assert.strictEqual(native.closed, 0);
			native.packets.push(...videoPackets(1));
			await timeout(8);
			host.close();
			assert.strictEqual(sent()[0]?.kind, 'video');
		}));
	});
	suite('files', () => {
		const listing = JSON.stringify({ ok: { entries: [], total: '0', nextOffset: null, truncated: false, skipped: '0' } });
		test('a slow file operation holds up neither the operations after it nor their answers', async () => {
			const { host, native, messages, receive } = channel('files');
			const slowListing = new DeferredPromise<string>();
			const operations: string[] = [];
			native.fileOperation = command => {
				const operation = (JSON.parse(command) as { type: string }).type;
				operations.push(operation);
				return operation === 'list' ? slowListing.p : Promise.resolve('{"error":"FileExists: the name is taken"}');
			};
			host.start(native);
			receive({ type: 'file', id: 1, operation: { type: 'list', root: '0', path: 'Users', offset: '0' } });
			receive({ type: 'file', id: 2, operation: { type: 'rename', root: '0', path: 'Users', name: 'a.txt', token: null, newName: 'b.txt' } });
			await timeout(0);
			assert.deepStrictEqual(operations, ['list', 'rename']);
			assert.deepStrictEqual(messages.filter(message => message.type === 'fileReply'), [{ type: 'fileReply', id: 2, metadata: '{"error":"FileExists: the name is taken"}' }]);
			await slowListing.complete(listing);
			await timeout(0);
			assert.deepStrictEqual(messages.filter(message => message.type === 'fileReply').map(message => message.id), [2, 1]);
		});
		test('a job change the engine pushes reaches the window as a packet', () => {
			const { host, native, sent } = channel('files');
			const jobs = JSON.stringify({ jobs: [] });
			native.packets.push({ kind: 'jobs', metadata: jobs });
			host.start(native);
			assert.deepStrictEqual(sent().map(message => [message.kind, message.metadata]), [['jobs', jobs]]);
		});
	});
	test('closing before the engine\'s end starts closes that late end exactly once', () => {
		const { host, native } = channel();
		host.close();
		host.start(native);
		host.close();
		assert.strictEqual(native.closed, 1);
	});
});
