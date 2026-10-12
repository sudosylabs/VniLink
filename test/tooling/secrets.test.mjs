import assert from 'node:assert/strict';
import test from 'node:test';
import { build } from 'esbuild';
import { fileURLToPath } from 'node:url';
const bundle = await build({ entryPoints: [fileURLToPath(new URL('../../src/platform/secrets/common/secrets.ts', import.meta.url))], bundle: true, write: false, format: 'esm', platform: 'node' });
const { SECRET_STORAGE_PREFIX, secretStorageKey, SequencerByKey, readEncryptedSecret, writeEncryptedSecret } = await import(`data:text/javascript;base64,${Buffer.from(bundle.outputFiles[0].text).toString('base64')}`);
test('secret keys carry the shared prefix', () => {
	assert.equal(SECRET_STORAGE_PREFIX, 'secret://');
	assert.equal(secretStorageKey('github.auth'), 'secret://github.auth');
});
test('per-key work serializes while other keys run free', async () => {
	const sequencer = new SequencerByKey();
	const order = [];
	const first = sequencer.queue('a', async () => {
		await new Promise(resolve => setTimeout(resolve, 20));
		order.push('first');
	});
	const second = sequencer.queue('a', async () => {
		order.push('second');
	});
	const other = sequencer.queue('b', async () => {
		order.push('other');
	});
	await Promise.all([first, second, other]);
	assert.deepEqual([...order].sort(), ['first', 'other', 'second']);
	assert.ok(order.indexOf('first') < order.indexOf('second'));
});
test('secrets round-trip through encrypt and prefixed storage', async () => {
	const store = new Map();
	const encrypt = async (value) => `enc:${value}`;
	const decrypt = async (value) => {
		assert.match(value, /^enc:/);
		return value.slice(4);
	};
	await writeEncryptedSecret('token', 's3cret', (key, value) => void store.set(key, value), encrypt);
	assert.equal(store.get('secret://token'), 'enc:s3cret');
	assert.equal(await readEncryptedSecret('token', key => store.get(key), decrypt), 's3cret');
	assert.equal(await readEncryptedSecret('missing', key => store.get(key), decrypt), undefined);
});
