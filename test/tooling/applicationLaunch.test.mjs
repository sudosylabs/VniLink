import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';
import { build } from 'esbuild';

const product = JSON.parse(await readFile(new URL('../../product.json', import.meta.url), 'utf8'));

async function launchFixture(windows) {
	const output = await build({
		entryPoints: ['src/code/electron-main/main.ts'], bundle: true, write: false, platform: 'node', format: 'esm', plugins: [{
			name: 'launch-fixture', setup(builder) {
				builder.onResolve({ filter: /^(electron|.*\/app\.js|.*\/platform\.js)$/ }, args => ({ path: args.path, namespace: 'fixture' }));
				builder.onLoad({ filter: /.*/, namespace: 'fixture' }, args => ({ contents: args.path === 'electron' ? `export const app = { setName: value => globalThis.launchEvents.push(['name', value]), setAppUserModelId: value => globalThis.launchEvents.push(['id', value]), setPath: (...values) => globalThis.launchEvents.push(['path', ...values]) };` : args.path.endsWith('/platform.js') ? `export const isWindows = ${windows};` : `export class VniLinkApplication { constructor(options) { globalThis.launchEvents.push(['construct', options]); } async start() { globalThis.launchEvents.push(['start']); } }` }));
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
		assert.deepEqual(globalThis.launchEvents, [['name', product.nameLong], ['id', product.appId], ['construct', { temporaryEngine: false }], ['start']]);
		globalThis.launchEvents = [];
		process.argv = ['vnilink', '--smoke-test'];
		delete process.env.VNILINK_TEST_PROFILE;
		await assert.rejects(new VniLinkMain().main(), /requires an isolated/);
		assert(!globalThis.launchEvents.some(event => event[0] === 'construct'));
		globalThis.launchEvents = [];
		process.env.VNILINK_TEST_PROFILE = '/temporary/profile';
		await new VniLinkMain().main();
		assert.deepEqual(globalThis.launchEvents.slice(2), [['path', 'userData', '/temporary/profile'], ['construct', { temporaryEngine: true }], ['start']]);
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
