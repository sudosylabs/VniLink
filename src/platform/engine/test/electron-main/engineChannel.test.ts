// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'assert';
import type { MessagePortMain } from 'electron';
import { EventEmitter } from 'events';
import { ensureNoDisposablesAreLeakedInTestSuite } from '../../../../base/test/common/utils.js';
import { TestInstantiationService } from '../../../instantiation/test/common/instantiationServiceMock.js';
import { INativeHostMainService } from '../../../native/electron-main/nativeHostMainService.js';
import { IPermissionsMainService } from '../../../permissions/electron-main/permissionsMainService.js';
import type { DataChannelRequest } from '../../common/dataChannel.js';
import { DATA_CHANNEL_PORT_RESPONSE } from '../../common/engineService.js';
import { EngineChannel, type IChannelWindowContents, type IChannelWindows, type PortPair } from '../../electron-main/engineChannel.js';
import { IEngineMainService } from '../../electron-main/engineMainService.js';
import { TestEngineMainService } from './testEngineMainService.js';
import { upcastPartial } from '../../../../base/test/common/mock.js';

/** Commands only the main process may send; a window that sends one is refused. */
const privateCommands: readonly unknown[] = [
	{ type: 'shutdown' },
	{ type: 'subscribeEvents' }, { type: 'pollEvents', subscriptionId: '1', maxEvents: 1 }, { type: 'releaseEvents', subscriptionId: '1' },
	{ type: 'setRelayPublic' }, { type: 'setSharingPolicy', policy: { view: true } }, { type: 'setDesktopCodecDefault', codec: 'auto' }, { type: 'setTransferDirectory', path: '/tmp/Transfers' }, { type: 'setTerminalShell', shell: null },
	{ type: 'setLogFilter', directives: 'off' },
	{ type: 'reportNativePermissions', screen: true, input: true, camera: true, microphone: true },
	{ type: 'resetIdentity' },
];

const request: DataChannelRequest = { requestId: 'probe', kind: 'video', sessionId: '1', generation: '1' };

/** A window's web contents, whose native listener hooks are gone once they are destroyed, as Electron's are. */
class TestWebContents extends EventEmitter implements IChannelWindowContents {
	public destroyed = false;
	public readonly posted: (readonly [string, unknown])[] = [];

	public isDestroyed(): boolean {
		return this.destroyed;
	}

	public postMessage(channel: string, message: unknown): void {
		this.posted.push([channel, message]);
	}

	public override removeListener(name: string, listener: Parameters<EventEmitter['removeListener']>[1]): this {
		if (this.destroyed && name === 'did-start-navigation') {
			throw new Error('Object has been destroyed');
		}
		return super.removeListener(name, listener);
	}
}

/** A port that is never connected; the engine double keeps no port. */
const port = upcastPartial<MessagePortMain>({
	close: () => { /* Nothing is connected to it. */ },
});

suite('EngineChannel', () => {
	const disposables = ensureNoDisposablesAreLeakedInTestSuite();
	let engine: TestEngineMainService;
	let contents: TestWebContents;

	/** The channel, on a main process whose only window is window 4. */
	function createChannel(): EngineChannel {
		const instantiationService = disposables.add(new TestInstantiationService());
		const window = { id: 4, webContents: contents };
		const windows: IChannelWindows = { getWindowById: id => id === 4 ? window : undefined, getMainWindow: () => window };
		instantiationService.stub(IEngineMainService, engine);
		instantiationService.stub(INativeHostMainService, {});
		instantiationService.stub(IPermissionsMainService, {});
		const ports = (): PortPair => ({ port1: port, port2: port });
		return disposables.add(new EngineChannel(
			instantiationService.get(IEngineMainService),
			windows,
			instantiationService.get(INativeHostMainService),
			instantiationService.get(IPermissionsMainService),
			ports,
		));
	}
	setup(() => {
		engine = disposables.add(new TestEngineMainService(() => ({ type: 'done' })));
		contents = new TestWebContents();
	});
	for (const command of privateCommands) {
		test(`a window cannot send ${JSON.stringify(command)}, which only main may send`, async () => {
			await assert.rejects(createChannel().call('window:1', 'execute', command), /Invalid/);
			assert.deepStrictEqual(engine.commands, []);
		});
	}
	test('a window\'s snapshot request reaches the engine and its reply the window', async () => {
		assert.deepStrictEqual(await createChannel().call('window:1', 'execute', { type: 'snapshot' }), { type: 'done' });
		assert.deepStrictEqual(engine.commands, [{ type: 'snapshot' }]);
	});
	test('a data channel is attached for the window that asked, and its port sent to that window', async () => {
		await createChannel().call('window:4', 'openDataChannel', request);
		assert.deepStrictEqual(engine.attached.map(attached => attached.request), [request]);
		assert.deepStrictEqual(contents.posted, [[DATA_CHANNEL_PORT_RESPONSE, 'probe']]);
	});
	test('a data channel for a window main does not know is refused', async () => {
		await assert.rejects(createChannel().call('window:5', 'openDataChannel', request));
		assert.deepStrictEqual(engine.attached, []);
	});
	test('destroyed contents release the window\'s channels without touching native listener hooks', async () => {
		await createChannel().call('window:4', 'openDataChannel', request);
		contents.destroyed = true;
		assert.doesNotThrow(() => contents.emit('destroyed'));
		assert.deepStrictEqual(engine.closedOwners, [engine.attached[0]?.owner]);
	});
	test('a window\'s channels are released once', async () => {
		await createChannel().call('window:4', 'openDataChannel', request);
		contents.destroyed = true;
		contents.emit('destroyed');
		contents.emit('destroyed');
		assert.strictEqual(engine.closedOwners.length, 1);
	});
});
