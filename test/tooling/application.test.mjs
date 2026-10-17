import assert from 'node:assert/strict';
import test from 'node:test';
import { build } from 'esbuild';

const output = await build({ stdin: { contents: `export { ApplicationService } from './src/workbench/services/application/electron-browser/applicationService.js'; export { Emitter } from './src/base/common/event.js'; export { relayDraftChanged } from './src/workbench/services/connection/common/relayConfiguration.js';`, resolveDir: process.cwd(), loader: 'ts' }, bundle: true, write: false, platform: 'node', format: 'esm', tsconfig: 'tsconfig.json' });

const { ApplicationService, Emitter, relayDraftChanged } = await import(`data:text/javascript;base64,${Buffer.from(output.outputFiles[0].text).toString('base64')}`);

test('restart state follows the engine and restart and documentation go through the native host', async () => {
	const restartRequired = new Emitter();
	const engine = { restartRequired: false, onDidChangeRestartRequired: restartRequired.event };
	const native = [];
	const service = new ApplicationService(engine, {
		relaunch: async () => {
			native.push('relaunch');
		},
		openExternal: async url => {
			native.push(url);
		},
	});
	let changes = 0;
	service.onDidChangeRestartRequired(() => changes++);
	assert(!service.restartRequired);
	engine.restartRequired = true;
	restartRequired.fire(true);
	assert(service.restartRequired);
	assert.equal(changes, 1);
	await service.restart();
	await service.openRelayDocumentation();
	assert.deepEqual(native, ['relaunch', 'https://docs.iroh.computer/relays/rate-limiting']);
	service.dispose();
	restartRequired.fire(false);
	assert.equal(changes, 1, 'A disposed service kept listening');
});

test('relay drafts compare normalized settings and count new credentials as an edit', () => {
	const saved = [{ url: 'https://relay.example.com/', authenticated: true }];
	assert(!relayDraftChanged(saved, { mode: 'custom', relays: [{ url: 'https://RELAY.example.com', authenticated: true, replacementToken: '' }] }));
	assert(relayDraftChanged(saved, { mode: 'custom', relays: [{ url: 'https://relay.example.com/', authenticated: true, replacementToken: 'new-token' }] }));
	assert(relayDraftChanged(saved, { mode: 'public' }));
	assert(!relayDraftChanged([], { mode: 'public' }));
});
