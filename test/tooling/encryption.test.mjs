import assert from 'node:assert/strict';
import test from 'node:test';
import { build } from 'esbuild';
import { fileURLToPath } from 'node:url';

const output = await build({
	entryPoints: [fileURLToPath(new URL('../../src/platform/encryption/electron-main/encryptionMainService.ts', import.meta.url))],
	bundle: true, write: false, format: 'esm', platform: 'node',
	plugins: [{
		name: 'safe-storage-fixture', setup(builder) {
			builder.onResolve({ filter: /^electron$/ }, () => ({ path: 'electron', namespace: 'fixture' }));
			builder.onLoad({ filter: /.*/, namespace: 'fixture' }, () => ({
				contents: `export const app = { commandLine: { getSwitchValue: () => '' } }; export const safeStorage = { encryptString: value => Buffer.from(value), decryptString: bytes => bytes.toString() };`,
				loader: 'js',
			}));
		}
	}],
});

const { EncryptionMainService } = await import(`data:text/javascript;base64,${Buffer.from(output.outputFiles[0].text).toString('base64')}`);

test('malformed encrypted values never appear in errors or logs', async () => {
	const logs = [];
	const service = new EncryptionMainService({ trace: (...args) => logs.push(args), error: (...args) => logs.push(args) });
	const marker = 'private-test-token';
	for (const value of [marker, JSON.stringify({ secret: marker }), JSON.stringify({ data: { secret: marker } })]) {
		await assert.rejects(service.decrypt(value), error => {
			assert.equal(error.message, '[EncryptionMainService] Invalid encrypted value');
			assert.equal(error.message.includes(marker), false);
			return true;
		});
	}
	assert.equal(JSON.stringify(logs).includes(marker), false);
});

test('valid encrypted buffer JSON still round-trips through the adapter', async () => {
	const service = new EncryptionMainService({ trace() { }, error() { } });
	assert.equal(await service.decrypt(await service.encrypt('round-trip')), 'round-trip');
});
