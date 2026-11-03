// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'node:assert/strict';
import { app, net } from 'electron';
import Mocha from 'mocha';
import { contentSecurityPolicy } from '../../../../../src/platform/window/common/contentSecurityPolicy.js';
import type { EngineReply } from '../../../../../src/platform/engine/common/engine.js';
import { ENGINE_CHANNEL_NAME } from '../../../../../src/platform/engine/common/engineService.js';
import type { Automation } from '../../../../automation/src/automation.js';

/** Whether `call` is refused. */
async function refused(call: () => Promise<unknown>): Promise<boolean> {
	try {
		await call();
		return false;
	} catch {
		return true;
	}
}

/** Turns hosting off through the saved setting, waits for the window to hear it, and turns it back on. */
async function pauseSharing(automation: Automation): Promise<void> {
	const renderer = automation.driver.renderer;
	// The window listens before the setting changes: calls reach the page in order.
	const heard = renderer.whenHosting(false);
	await renderer.updateSetting('application.acceptConnections', false);
	await heard;
	await renderer.updateSetting('application.acceptConnections', true);
}

/** The native engine as a window reaches it, and its restart after a crash. */
export function setup(automation: Automation): void {
	const renderer = automation.driver.renderer;
	Mocha.describe('engine-restart', () => {
		Mocha.it('tells a window the engine is coming back before it is gone, and the engine hosts again', async () => {
			const engine = app.getAppMetrics().find(metric => metric.type === 'Utility' && metric.name === 'VniLink Engine');
			assert(engine, 'the engine process is not running');
			// The window listens before the process is stopped, so nothing it is told is missed.
			await renderer.listenForEngineRestart();
			process.kill(engine.pid, 'SIGKILL');
			const { story, restarting } = await renderer.engineRestarted();
			const snapshot = await renderer.execute({ type: 'snapshot' });
			await pauseSharing(automation);
			assert.deepEqual(story.slice(0, 2), ['restarting', 'unavailable'], 'windows were not told the engine was coming back before they heard it was gone');
			assert.deepEqual(story.slice(-2), ['steady', 'available']);
			assert(snapshot.type === 'snapshot' && snapshot.snapshot.hosting, 'the restarted engine does not host again');
			assert.equal(restarting, false);
		});
	});
	Mocha.describe('engine', () => {
		let snapshot: EngineReply;
		Mocha.it('answers a snapshot with this computer\'s 64-character endpoint, hosting', async () => {
			snapshot = await renderer.execute({ type: 'snapshot' });
			assert(snapshot.type === 'snapshot');
			assert.equal(snapshot.snapshot.endpoint_id.length, 64);
			assert.equal(snapshot.snapshot.hosting, true);
		});
		Mocha.it('creates an invitation with a token', async () => {
			const invitation = await renderer.execute({ type: 'createInvitation', ttlSeconds: 60 });
			assert(invitation.type === 'invitation' && invitation.token, 'no invitation token');
		});
		Mocha.it('refuses a window the commands only main may send', async () => {
			// Sent on the engine channel as `IEngineService.execute` sends them, untyped as a page could.
			const execute = (type: string) => renderer.callMainProcess(ENGINE_CHANNEL_NAME, 'execute', { type });
			assert.equal(await refused(() => execute('shutdown')), true, 'a window shut the engine down');
			assert.equal(await refused(() => execute('subscribeEvents')), true, 'a window subscribed to the engine\'s private events');
			assert.equal(await refused(() => execute('stopHosting')), true, 'a window stopped hosting');
		});
		Mocha.it('reports an engine error to the window and keeps answering', async () => {
			const error = await renderer.execute({ type: 'disconnect' }).then(() => '', (failure: unknown) => String(failure));
			assert.match(error, /NoSession/);
			assert.equal((await renderer.execute({ type: 'snapshot' })).type, 'snapshot');
		});
		Mocha.it('turns hosting off when sharing is paused, and the window hears it', async () => {
			await pauseSharing(automation);
		});
		Mocha.it('refuses a channel main does not offer, and a window action it does not know', async () => {
			assert.equal(await refused(() => renderer.callMainProcess('encryption', 'encrypt', 'secret')), true, 'an unknown channel answered');
			assert.equal(await refused(async () => renderer.callMainProcess('nativeHost', 'invalid', [await renderer.getWindowId()])), true, 'an invalid window action was accepted');
		});
		Mocha.it('serves only listed assets, each document with its content security policy', async () => {
			assert.equal((await net.fetch('vnilink://app/main.cjs')).status, 404, 'an unlisted asset is served');
			// The policy travels as a header, which `frame-ancestors` needs.
			const policy = (await net.fetch('vnilink://app/index.html')).headers.get('content-security-policy');
			assert.equal(policy, contentSecurityPolicy, 'the workbench is served without its content security policy');
			assert(policy.includes('frame-ancestors \'none\''));
		});
	});
}
