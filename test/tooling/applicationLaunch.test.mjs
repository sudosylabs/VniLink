// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';
import { build } from 'esbuild';

const product = JSON.parse(await readFile(new URL('../../product.json', import.meta.url), 'utf8'));

async function launchFixture(windows) {
	const output = await build({
		entryPoints: ['src/code/electron-main/main.ts'], bundle: true, write: false, platform: 'node', format: 'esm', plugins: [{
			name: 'launch-fixture', setup(builder) {
				builder.onResolve({ filter: /^(electron|.*\/app\.js|.*\/platform\.js|.*\/environment\.js|.*\/selfCheck\.js)$/ }, args => ({ path: args.path, namespace: 'fixture' }));
				builder.onLoad({ filter: /.*/, namespace: 'fixture' }, args => ({ contents: args.path === 'electron' ? `export const app = { setName: value => globalThis.launchEvents.push(['name', value]), setAppUserModelId: value => globalThis.launchEvents.push(['id', value]), setPath: (...values) => globalThis.launchEvents.push(['path', ...values]), exit: code => globalThis.launchEvents.push(['exit', code]), commandLine: { hasSwitch: name => (globalThis.launchSwitches ?? []).includes(name) } };` : args.path.endsWith('/platform.js') ? `export const isWindows = ${windows};` : args.path.endsWith('/environment.js') ? `export function isBuilt() { return globalThis.launchBuilt === true; }` : args.path.endsWith('/selfCheck.js') ? `export function selfCheckOutput() { return undefined; }` : `export class VniLinkApplication { constructor(options) { globalThis.launchEvents.push(['construct', options]); } async start() { globalThis.launchEvents.push(['start']); } }` }));
			}
		}]
	});
	return await import(`data:text/javascript;base64,${Buffer.from(output.outputFiles[0].text).toString('base64')}`);
}

test('launch establishes identity before construction and smoke isolation is explicit and required', async () => {
	const argv = process.argv;
	const profile = process.env.VNILINK_TEST_PROFILE;
	try {
		globalThis.launchEvents = [];
		process.argv = ['vnilink'];
		const { VniLinkMain } = await launchFixture(true);
		await new VniLinkMain().main();
		assert.deepEqual(globalThis.launchEvents, [['name', product.nameLong], ['id', product.appId], ['construct', { temporaryEngine: false, automationDriver: false }], ['start']]);
		globalThis.launchEvents = [];
		process.argv = ['vnilink', '--smoke-test'];
		delete process.env.VNILINK_TEST_PROFILE;
		await assert.rejects(new VniLinkMain().main(), /requires an isolated/);
		assert(!globalThis.launchEvents.some(event => event[0] === 'construct'));
		globalThis.launchEvents = [];
		process.env.VNILINK_TEST_PROFILE = '/temporary/profile';
		await new VniLinkMain().main();
		assert.deepEqual(globalThis.launchEvents.slice(2), [['path', 'userData', '/temporary/profile'], ['construct', { temporaryEngine: true, automationDriver: true }], ['start']]);
	} finally {
		process.argv = argv;
		if (profile === undefined) {
			delete process.env.VNILINK_TEST_PROFILE;
		} else {
			process.env.VNILINK_TEST_PROFILE = profile;
		}
		delete globalThis.launchEvents;
	}
});

test('a built application refuses the smoke-test driver and remote debugging before it starts anything', async () => {
	const argv = process.argv;
	try {
		globalThis.launchBuilt = true;
		const { VniLinkMain } = await launchFixture(false);
		for (const [args, switches] of [[['vnilink', '--smoke-test'], []], [['vnilink'], ['remote-debugging-port']], [['vnilink'], ['remote-debugging-pipe']]]) {
			globalThis.launchEvents = [];
			globalThis.launchSwitches = switches;
			process.argv = args;
			await new VniLinkMain().main();
			assert.deepEqual(globalThis.launchEvents.slice(1), [['exit', 2]], `${args.join(' ')} ${switches.join(' ')} was accepted`);
		}
	} finally {
		process.argv = argv;
		delete globalThis.launchBuilt;
		delete globalThis.launchSwitches;
		delete globalThis.launchEvents;
	}
});
