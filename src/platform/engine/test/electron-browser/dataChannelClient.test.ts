// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'assert';
import { DeferredPromise, timeout } from '../../../../base/common/async.js';
import { toDisposable } from '../../../../base/common/lifecycle.js';
import { ensureNoDisposablesAreLeakedInTestSuite } from '../../../../base/test/common/utils.js';
import type { DataPacket, DesktopAudioStatus } from '../../common/dataChannel.js';
import type { DesktopInputCommand } from '../../common/engine.js';
import type { FileJob } from '../../common/files.js';
import { DataChannelClient } from '../../electron-browser/dataChannelClient.js';

/** A message one end of the channel posted to the other. */
interface Posted {
	readonly type: string;
	readonly id?: number;
	readonly [field: string]: unknown;
}

/**
 * The engine host's end of a channel: it keeps what the window posted, and
 * `next` waits for the first message, already arrived or still to come, that
 * matches.
 */
class TestHostPort {
	public readonly messages: Posted[] = [];
	private readonly waiting: { readonly matches: (message: Posted) => boolean; readonly arrived: DeferredPromise<Posted> }[] = [];

	constructor(private readonly port: MessagePort, private readonly answer: (message: Posted, host: TestHostPort) => void = () => undefined) {
		port.onmessage = event => {
			const message = event.data as Posted;
			this.messages.push(message);
			for (const waiter of this.waiting.filter(entry => entry.matches(message))) {
				this.waiting.splice(this.waiting.indexOf(waiter), 1);
				void waiter.arrived.complete(message);
			}
			this.answer(message, this);
		};
		port.start();
	}

	public next(matches: (message: Posted) => boolean): Promise<Posted> {
		const arrived = this.messages.find(matches);
		if (arrived) {
			return Promise.resolve(arrived);
		}
		const waiter = { matches, arrived: new DeferredPromise<Posted>() };
		this.waiting.push(waiter);
		return waiter.arrived.p;
	}

	public post(message: unknown): void {
		this.port.postMessage(message);
	}

	public close(): void {
		this.port.close();
	}
}

/** A packet of `kind` the host sends as the channel's `sequence`th. */
function dataMessage(sequence: number, kind = 'video', fields: Record<string, unknown> = {}): Record<string, unknown> {
	return { type: 'data', sequence, kind, data: new Uint8Array([7]), metadata: '{}', ...fields };
}

function pointer(x: number): DesktopInputCommand {
	return { type: 'submitPointer', sessionId: '1', generation: '2', x, y: 0, display: '1', revision: '1' };
}

const key: DesktopInputCommand = { type: 'submitKey', sessionId: '1', generation: '2', usage: 4, down: true };

const job: FileJob = {
	id: '7', peer: 'p', label: 'a.txt', direction: 'Upload', sender: true, state: 'Transferring', total_bytes: '100', done_bytes: '40',
	files_total: '1', files_done: '0', resumed_files: '0', skipped_files: '0', skipped_symlinks: '0', waiting_file: null, current_file: null, error: null, destination: null,
};

const audioStatus: DesktopAudioStatus = { phase: 'muted', enabled: true, muted: true, playbackFailed: false, error: null };

suite('DataChannelClient', () => {
	const disposables = ensureNoDisposablesAreLeakedInTestSuite();
	let errors: Error[];

	/**
	 * A client on one end of a fresh port pair and the host on the other;
	 * `answer` lets the host reply to what the client posts.
	 */
	function createChannel(
		consume: (packet: DataPacket) => void | Promise<void>,
		answer?: (message: Posted, host: TestHostPort) => void,
	): { client: DataChannelClient; host: TestHostPort } {
		const { port1, port2 } = new MessageChannel();
		const host = new TestHostPort(port2, answer);
		const client = new DataChannelClient(port1, consume, error => errors.push(error));
		disposables.add(toDisposable(() => {
			client.close();
			host.close();
		}));
		return { client, host };
	}

	/** The host acknowledges each input as written. */
	function acknowledgeInput(message: Posted, host: TestHostPort): void {
		if (message.type === 'input') {
			host.post({ type: 'written', id: message.id });
		}
	}
	setup(() => {
		errors = [];
	});
	suite('packets', () => {
		test('a packet is acknowledged only once it was consumed', async () => {
			const consuming = new DeferredPromise<void>();
			const entered = new DeferredPromise<void>();
			const { client, host } = createChannel(() => {
				void entered.complete();
				return consuming.p;
			});
			host.post(dataMessage(1));
			await entered.p;
			// Messages on a port keep their order: an early acknowledgement would arrive before this request.
			client.requestKeyframe();
			await consuming.complete();
			await host.next(message => message.type === 'ack');
			assert.deepStrictEqual(host.messages.map(message => message.type), ['keyframe', 'ack']);
			assert.deepStrictEqual(errors, []);
		});
		test('a packet keeps the time the engine read it', async () => {
			const consumed = new DeferredPromise<DataPacket>();
			const { host } = createChannel(packet => {
				void consumed.complete(packet);
			});
			host.post(dataMessage(1, 'video', { readAt: 1234.5 }));
			assert.strictEqual((await consumed.p).readAt, 1234.5);
		});
		test('a malformed read time fails the channel without reaching the consumer', async () => {
			const consumed: DataPacket[] = [];
			const failed = new DeferredPromise<void>();
			const { port1, port2 } = new MessageChannel();
			const host = new TestHostPort(port2);
			const client = new DataChannelClient(port1, packet => {
				consumed.push(packet);
			}, error => {
				errors.push(error);
				void failed.complete();
			});
			disposables.add(toDisposable(() => {
				client.close();
				host.close();
			}));
			host.post(dataMessage(1, 'video', { readAt: 'now' }));
			await failed.p;
			assert.deepStrictEqual(consumed, []);
			assert.match(errors[0].message, /Invalid data sequence/);
		});
	});
	suite('input', () => {
		test('pointer moves waiting to be sent collapse into the latest, without losing a key between them', async () => {
			const { client, host } = createChannel(() => undefined, acknowledgeInput);
			client.input(pointer(1));
			client.input(pointer(2));
			client.input(key);
			client.input(pointer(3));
			await host.next(message => message.type === 'input' && (message.command as DesktopInputCommand).type === 'submitPointer' && (message.command as { x: number }).x === 3);
			const inputs = host.messages.filter(message => message.type === 'input').map(message => message.command as DesktopInputCommand);
			assert.deepStrictEqual(inputs, [pointer(2), key, pointer(3)]);
			assert.deepStrictEqual(errors, []);
		});
		test('closing with input not yet acknowledged reports no failure', async () => {
			const { client, host } = createChannel(() => undefined);
			client.input(key);
			await host.next(message => message.type === 'input');
			client.close();
			await timeout(0);
			assert.deepStrictEqual(errors, []);
		});
	});
	suite('files', () => {
		test('a slow file operation holds up neither the operations after it nor their answers', async () => {
			const { client, host } = createChannel(() => undefined, (message, port) => {
				const operation = message.type === 'file' ? (message.operation as { type: string }).type : undefined;
				if (operation === 'rename') {
					port.post({ type: 'fileReply', id: message.id, metadata: '{"error":"FileExists: the name is taken"}' });
				} else if (operation === 'cancel') {
					port.post({ type: 'fileReply', id: message.id, metadata: JSON.stringify({ ok: { ...job, state: 'Cancelled' } }) });
				}
			});
			const list = client.fileOperation({ type: 'list', root: '0', path: 'Users', offset: '0' });
			assert.deepStrictEqual(await client.fileOperation({ type: 'rename', root: '0', path: 'Users', name: 'a.txt', token: null, newName: 'b.txt' }), { error: 'FileExists: the name is taken' });
			assert.deepStrictEqual(await client.fileOperation({ type: 'cancel', jobId: '7' }), { ok: { ...job, state: 'Cancelled' } });
			const listing = await host.next(message => message.type === 'file' && (message.operation as { type: string }).type === 'list');
			host.post({ type: 'fileReply', id: listing.id, metadata: JSON.stringify({ ok: { entries: [], total: '0', nextOffset: null, truncated: false, skipped: '0' } }) });
			assert.deepStrictEqual(await list, { ok: { entries: [], total: '0', nextOffset: null, truncated: false, skipped: '0' } });
			assert.deepStrictEqual(host.messages.filter(message => message.type === 'file').map(message => (message.operation as { type: string }).type), ['list', 'rename', 'cancel']);
		});
		test('a refused file operation is an answer, not a failure', async () => {
			const { client } = createChannel(() => undefined, (message, port) => {
				if (message.type === 'file') {
					port.post({ type: 'fileReply', id: message.id, metadata: '{"error":"FileExists: the name is taken"}' });
				}
			});
			assert.deepStrictEqual(await client.fileOperation({ type: 'rename', root: '0', path: 'Users', name: 'a.txt', token: null, newName: 'b.txt' }), { error: 'FileExists: the name is taken' });
			assert.deepStrictEqual(errors, []);
		});
		test('a job change arrives as a packet nobody asked for', async () => {
			const consumed = new DeferredPromise<DataPacket>();
			const { host } = createChannel(packet => {
				void consumed.complete(packet);
			});
			host.post({ type: 'data', sequence: 1, kind: 'jobs', metadata: JSON.stringify({ jobs: [job] }) });
			const packet = await consumed.p;
			assert.strictEqual(packet.kind, 'jobs');
			assert.deepStrictEqual(JSON.parse(packet.metadata), { jobs: [job] });
		});
	});
	suite('desktop audio', () => {
		test('a request is answered by its own reply, not by an older one', async () => {
			const { client, host } = createChannel(() => undefined);
			const pending = client.desktopAudio({ type: 'muted', muted: true });
			const asked = await host.next(message => message.type === 'desktopAudio');
			host.post({ type: 'desktopAudio', id: (asked.id ?? 0) + 100, status: { ...audioStatus, muted: false } });
			host.post({ type: 'desktopAudio', id: asked.id, status: audioStatus });
			assert.deepStrictEqual(await pending, audioStatus);
		});
		test('a reply that comes again leaves the channel running', async () => {
			const consumed = new DeferredPromise<void>();
			const { client, host } = createChannel(() => {
				void consumed.complete();
			});
			const pending = client.desktopAudio({ type: 'muted', muted: true });
			const asked = await host.next(message => message.type === 'desktopAudio');
			host.post({ type: 'desktopAudio', id: asked.id, status: audioStatus });
			await pending;
			host.post({ type: 'desktopAudio', id: asked.id, status: audioStatus });
			host.post(dataMessage(1));
			await consumed.p;
			assert.deepStrictEqual(errors, []);
		});
		test('closing rejects a request still waiting for its reply', async () => {
			const { client } = createChannel(() => undefined);
			const pending = client.desktopAudio({ type: 'status' });
			client.close();
			await assert.rejects(pending, /closed/);
		});
		test('a closed channel refuses requests', async () => {
			const { client } = createChannel(() => undefined);
			client.close();
			await assert.rejects(client.desktopAudio({ type: 'status' }), /unavailable/);
		});
	});
});
