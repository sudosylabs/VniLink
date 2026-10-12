import assert from 'node:assert/strict';
import test from 'node:test';
import { build } from 'esbuild';
import { fileURLToPath } from 'node:url';
const bundle = await build({ entryPoints: [fileURLToPath(new URL('../../src/platform/engine/common/engine.ts', import.meta.url))], bundle: true, write: false, format: 'esm', platform: 'node' });
const { isEngineCommand, isEngineReply } = await import(`data:text/javascript;base64,${Buffer.from(bundle.outputFiles[0].text).toString('base64')}`);
test('control commands reject privileged methods, malformed IDs and extra authority', () => {
	assert.equal(isEngineCommand({ type: 'shutdown' }), false);
	assert.equal(isEngineCommand({ type: 'snapshot', extra: true }), false);
	assert.equal(isEngineCommand({ type: 'confirmPairing', previewId: '18446744073709551615', digits: '123' }), true);
	assert.equal(isEngineCommand({ type: 'confirmPairing', previewId: '18446744073709551616', digits: '123' }), false);
	assert.equal(isEngineCommand({ type: 'confirmPairing', previewId: 1, digits: '123' }), false);
	assert.equal(isEngineCommand({ type: 'confirmPairing', previewId: '1', digits: '123456' }), false);
	assert.equal(isEngineCommand({ type: 'openPairingFile', bytes: [256] }), false);
	assert.equal(isEngineCommand({ type: 'createInvitation', ttlSeconds: 60 }), true);
	assert.equal(isEngineCommand({ type: 'createInvitation', ttlSeconds: NaN }), false);
	assert.equal(isEngineCommand({ type: 'authorize', endpointId: 'peer', view: true, control: true, clipboard: false, transfer: false }), true);
	assert.equal(isEngineCommand({ type: 'authorize', endpointId: 'peer', view: true, control: true, clipboard: false }), false);
});
test('pairing handles cross the bridge as lossless decimal strings', () => {
	const preview = { id: '18446744073709551615', endpoint_id: 'peer', check: '123', expires_at_ms: '18446744073709551615' };
	assert.equal(isEngineReply({ type: 'pairingPreview', preview }), true);
	assert.equal(isEngineReply({ type: 'pairingPreview', preview: { ...preview, id: Number(preview.id) } }), false);
	assert.equal(isEngineReply({ type: 'device', device: { endpoint_id: 'peer' } }), false);
});
