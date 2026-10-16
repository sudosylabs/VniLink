import assert from 'node:assert/strict';
import test from 'node:test';
import { build } from 'esbuild';

const output = await build({ stdin: { contents: `export { ApplicationService } from './src/workbench/services/application/electron-browser/applicationService.js'; export { isRestartState } from './src/platform/application/common/application.js'; export { relayDraftChanged } from './src/workbench/services/connection/common/relayConfiguration.js';`, resolveDir: process.cwd(), loader: 'ts' }, bundle: true, write: false, platform: 'node', format: 'esm', tsconfig: 'tsconfig.json' });

const { ApplicationService, isRestartState, relayDraftChanged } = await import(`data:text/javascript;base64,${Buffer.from(output.outputFiles[0].text).toString('base64')}`);

test('restart state follows current facts across pages and ignores stale initial reads', async () => {
	let receive;
	let finish;
	let unsubscribed = false;
	let restarts = 0;
	const service = new ApplicationService({
		getRestartState: () => new Promise(resolve => {
			finish = resolve;
		}), onRestartStateChanged: listener => {
			receive = listener;
			return () => {
				unsubscribed = true;
			};
		}, restartApplication: async () => {
			restarts++;
		}, openRelayDocumentation: async () => { }
	});
	let changes = 0;
	service.onDidChangeRestartRequired(() => changes++);
	receive({ revision: 2, required: true });
	finish({ revision: 1, required: false });
	await Promise.resolve();
	assert(service.restartRequired);
	assert.equal(changes, 1);
	await service.restart();
	assert.equal(restarts, 1);
	receive({ revision: 3, required: false });
	assert(!service.restartRequired);
	service.dispose();
	receive({ revision: 4, required: true });
	assert(!service.restartRequired);
	assert(unsubscribed);
	assert(!isRestartState({ revision: 1, required: true, token: 'not-allowed' }));
});

test('relay drafts compare normalized settings and count new credentials as an edit', () => {
	const saved = [{ url: 'https://relay.example.com/', authenticated: true }];
	assert(!relayDraftChanged(saved, { mode: 'custom', relays: [{ url: 'https://RELAY.example.com', authenticated: true, replacementToken: '' }] }));
	assert(relayDraftChanged(saved, { mode: 'custom', relays: [{ url: 'https://relay.example.com/', authenticated: true, replacementToken: 'new-token' }] }));
	assert(relayDraftChanged(saved, { mode: 'public' }));
	assert(!relayDraftChanged([], { mode: 'public' }));
});
