import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import test from 'node:test';
const addon = createRequire(import.meta.url)('../../out/native/vnilink.node');
const execute = async (engine, command) => JSON.parse(await engine.execute(JSON.stringify(command)));
test('two Rust engines pair through a signed file, retain grants, and reject commands after shutdown', { timeout: 30000 }, async () => {
	const first = await addon.openTemporaryEngine();
	let second;
	try {
		second = await addon.openTemporaryEngine();
		const a = (await execute(first, { type: 'snapshot' })).snapshot;
		const b = (await execute(second, { type: 'snapshot' })).snapshot;
		assert.notEqual(a.endpoint_id, b.endpoint_id);
		assert.equal(a.hosting, false);
		assert.equal(b.devices.length, 0);
		const { file } = await execute(first, { type: 'exportPairingFile', ttlSeconds: 60, allowControl: false });
		assert.match(file.check, /^[0-9]{3}$/);
		const { preview } = await execute(second, { type: 'openPairingFile', bytes: file.bytes });
		assert.equal(preview.endpoint_id, a.endpoint_id);
		assert.equal(typeof preview.id, 'string');
		const { device } = await execute(second, { type: 'confirmPairing', previewId: preview.id, digits: file.check });
		assert.equal(device.endpoint_id, a.endpoint_id);
		assert.deepEqual(device.grants, { view: false, control: false, clipboard: false, transfer: false });
		await assert.rejects(async () => second.execute(JSON.stringify({ type: 'snapshot', unknown: true })));
		await assert.rejects(async () => second.execute(JSON.stringify({ type: 'publishVideo' })));
		await execute(second, { type: 'setPeerName', endpointId: a.endpoint_id, name: 'My computer' });
		assert.equal((await execute(second, { type: 'snapshot' })).snapshot.devices[0].pet_name, 'My computer');
		await execute(second, { type: 'authorize', endpointId: a.endpoint_id, view: true, control: false, clipboard: false, transfer: false });
		assert.deepEqual((await execute(second, { type: 'snapshot' })).snapshot.devices[0].grants, { view: true, control: false, clipboard: false, transfer: false });
		await execute(second, { type: 'revoke', endpointId: a.endpoint_id });
		await execute(second, { type: 'unpair', endpointId: a.endpoint_id });
		assert.equal((await execute(second, { type: 'snapshot' })).snapshot.devices.length, 0);
	} finally {
		await execute(first, { type: 'shutdown' });
		if (second) {
			await execute(second, { type: 'shutdown' });
		}
	}
	await assert.rejects(execute(first, { type: 'snapshot' }), /shut down/);
});
