// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'assert';
import type { Event } from '../../../../base/common/event.js';
import { toDisposable } from '../../../../base/common/lifecycle.js';
import type { IChannel } from '../../../../base/parts/ipc/common/ipc.js';
import { ensureNoDisposablesAreLeakedInTestSuite } from '../../../../base/test/common/utils.js';
import { SyncDescriptor } from '../../../instantiation/common/descriptors.js';
import { ServiceCollection } from '../../../instantiation/common/serviceCollection.js';
import { TestInstantiationService } from '../../../instantiation/test/common/instantiationServiceMock.js';
import { IMainProcessService } from '../../../ipc/common/mainProcessService.js';
import { TestMainProcessService } from '../../../ipc/test/common/testMainProcessService.js';
import { INativeHostService, NATIVE_HOST_CHANNEL_NAME, type IWindowState } from '../../common/native.js';
import { NativeHostService } from '../../common/nativeHostService.js';

const state: IWindowState = { windowId: 7, revision: 1, focused: false, maximized: false, fullscreen: false, alwaysOnTop: false, zoomFactor: 1, backgroundMaterial: 'none', accentColor: undefined };

/**
 * Main's native-host channel: records every call, answers `getWindowState`
 * with {@link state}, and `receive` delivers a pushed window state straight
 * to the window's listener.
 */
class TestNativeHostChannel implements IChannel {
	public readonly calls: [string, unknown][] = [];
	public receive: (state: IWindowState) => void = () => assert.fail('The window did not subscribe');
	public unsubscribed = 0;

	public call<T>(command: string, arg?: unknown): Promise<T> {
		this.calls.push([command, arg]);
		return Promise.resolve((command === 'getWindowState' ? state : undefined) as T);
	}

	public listen<T>(event: string): Event<T> {
		assert.strictEqual(event, 'onDidChangeWindowState');
		return listener => {
			this.receive = listener as (state: IWindowState) => void;
			return toDisposable(() => this.unsubscribed++);
		};
	}
}

suite('NativeHostService', () => {
	const disposables = ensureNoDisposablesAreLeakedInTestSuite();
	let channel: TestNativeHostChannel;
	let instantiationService: TestInstantiationService;

	/** The window's proxy, built as the workbench registers it for window 7. */
	function host(): INativeHostService {
		return instantiationService.invokeFunction(accessor => accessor.get(INativeHostService));
	}
	setup(() => {
		channel = new TestNativeHostChannel();
		instantiationService = disposables.add(new TestInstantiationService(new ServiceCollection(
			[IMainProcessService, new TestMainProcessService({ [NATIVE_HOST_CHANNEL_NAME]: channel })],
			[INativeHostService, new SyncDescriptor(NativeHostService, [7])],
		), true));
	});
	test('sends its window as the context of each call', async () => {
		const service = host();
		await service.focusWindow();
		await service.getWindowState();
		await service.pickFolder('/Pictures');
		assert.deepStrictEqual(channel.calls, [['focusWindow', [7]], ['getWindowState', [7]], ['pickFolder', [7, '/Pictures']]]);
	});
	test('returns main\'s answer', async () => {
		assert.deepStrictEqual(await host().getWindowState(), state);
	});
	test('forwards window state events', () => {
		const events: IWindowState[] = [];
		disposables.add(host().onDidChangeWindowState(value => events.push(value)));
		channel.receive({ ...state, revision: 3, focused: true });
		assert.deepStrictEqual(events, [{ ...state, revision: 3, focused: true }]);
	});
	test('answers its window id without asking main', () => {
		assert.strictEqual(host().windowId, 7);
		assert.deepStrictEqual(channel.calls, []);
	});
	test('releases main\'s event when its listener is disposed', () => {
		host().onDidChangeWindowState(() => undefined).dispose();
		assert.strictEqual(channel.unsubscribed, 1);
	});
	test('is not disposed over the channel when the container is', () => {
		host();
		instantiationService.dispose();
		assert.deepStrictEqual(channel.calls, []);
	});
});
