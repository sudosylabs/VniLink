// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'assert';
import { DeferredPromise } from '../../../../../base/common/async.js';
import type { Event } from '../../../../../base/common/event.js';
import { DisposableStore, toDisposable } from '../../../../../base/common/lifecycle.js';
import type { IChannel } from '../../../../../base/parts/ipc/common/ipc.js';
import { ensureNoDisposablesAreLeakedInTestSuite } from '../../../../../base/test/common/utils.js';
import { settingDefaults } from '../../../../../platform/configuration/common/configurationSchema.js';
import { CONFIGURATION_CHANNEL_NAME, type ConfigurationSnapshot } from '../../../../../platform/configuration/common/configurationSnapshot.js';
import { TestInstantiationService } from '../../../../../platform/instantiation/test/common/instantiationServiceMock.js';
import { IMainProcessService } from '../../../../../platform/ipc/common/mainProcessService.js';
import { TestMainProcessService } from '../../../../../platform/ipc/test/common/testMainProcessService.js';
import { ConfigurationService } from '../../configurationServiceImpl.js';

const quality = { max_bitrate_kbps: 2000, max_frame_rate: 30, max_width: 1920, max_height: 1080 };

function snapshot(revision: number, values: Record<string, unknown> = {}, errors: ConfigurationSnapshot['errors'] = {}): ConfigurationSnapshot {
	return { revision, values: { ...settingDefaults('user'), ...values }, errors, restricted: [] };
}

/**
 * Main's configuration channel: the first snapshot is answered by `first`,
 * every update fails, and `receive` delivers a pushed snapshot straight to
 * the window's listener.
 */
class TestConfigurationChannel implements IChannel {
	public readonly first = new DeferredPromise<ConfigurationSnapshot>();
	public receive: (snapshot: ConfigurationSnapshot) => void = () => assert.fail('The window did not subscribe');
	public unsubscribed = 0;

	public call<T>(command: string): Promise<T> {
		return command === 'getSnapshot' ? this.first.p as Promise<T> : Promise.reject(new Error('disk failure'));
	}

	public listen<T>(event: string): Event<T> {
		assert.strictEqual(event, 'onDidChangeConfiguration');
		return listener => {
			this.receive = listener as (snapshot: ConfigurationSnapshot) => void;
			return toDisposable(() => this.unsubscribed++);
		};
	}
}

suite('ConfigurationService', () => {
	const disposables = new DisposableStore();
	let channel: TestConfigurationChannel;
	let service: ConfigurationService;
	let loading: Promise<void>;
	setup(() => {
		channel = new TestConfigurationChannel();
		const instantiationService = disposables.add(new TestInstantiationService());
		instantiationService.stub(IMainProcessService, new TestMainProcessService({ [CONFIGURATION_CHANNEL_NAME]: channel }));
		service = disposables.add(instantiationService.createInstance(ConfigurationService));
		loading = service.initialize();
	});
	teardown(async () => {
		// The first read is always answered, so no test leaves it pending.
		await channel.first.complete(snapshot(0));
		await loading;
		disposables.clear();
	});
	test('a window keeps its own copy of a snapshot', () => {
		const sent = { ...quality };
		channel.receive(snapshot(2, { 'desktop.quality': sent }));
		sent.max_frame_rate = 10;
		assert.strictEqual(service.getValue<typeof quality>('desktop.quality').max_frame_rate, 30);
	});
	test('a source that cannot be read makes its settings unavailable, with the reason', () => {
		channel.receive(snapshot(2, {}, { power: 'Unavailable' }));
		assert.strictEqual(service.getUnavailableReason('power.lockAfterControl'), 'Unavailable');
	});
	test('a snapshot older than the one held is ignored', async () => {
		channel.receive(snapshot(2, { 'appearance.theme': 'dark' }));
		await channel.first.complete(snapshot(0));
		await loading;
		assert.strictEqual(service.getValue('appearance.theme'), 'dark');
	});
	test('a change in readability alone is announced for the settings it affects', () => {
		channel.receive(snapshot(2, { 'appearance.theme': 'dark' }, { power: 'Unavailable' }));
		const changes: string[][] = [];
		disposables.add(service.onDidChangeConfiguration(event => changes.push([...event.affectedKeys])));
		channel.receive(snapshot(3, { 'appearance.theme': 'dark' }));
		assert.strictEqual(changes.length, 1);
		assert.ok(changes[0].includes('power.lockAfterControl'));
		assert.ok(!changes[0].includes('appearance.theme'));
	});
	test('a failed update keeps the value the window holds', async () => {
		channel.receive(snapshot(2, { 'appearance.theme': 'dark' }));
		await assert.rejects(service.updateValue('appearance.theme', 'light'), /disk failure/);
		assert.strictEqual(service.getValue('appearance.theme'), 'dark');
	});
	test('disposing the service releases its subscription', () => {
		service.dispose();
		assert.strictEqual(channel.unsubscribed, 1);
	});
	test('a snapshot that arrives after disposal is ignored', () => {
		channel.receive(snapshot(2, { 'appearance.theme': 'dark' }));
		service.dispose();
		channel.receive(snapshot(4));
		assert.strictEqual(service.getValue('appearance.theme'), 'dark');
	});
	ensureNoDisposablesAreLeakedInTestSuite();
});
