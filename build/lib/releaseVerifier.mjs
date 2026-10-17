/** Checks the exact staged payload and loads its addon with an isolated temporary identity. */
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { createRequire } from 'node:module';
import { join } from 'node:path';
import { releaseInventory } from './releaseBuild.mjs';

export class ReleaseVerifier {
	constructor(directory) {
		this.directory = directory;
	}

	async verifyFiles() {
		const manifest = JSON.parse(await readFile(join(this.directory, 'build-manifest.json'), 'utf8'));
		assert.equal(manifest.profile, 'release', 'Expected a release profile');
		assert.equal(manifest.platform, process.platform, 'Release platform differs from the verifier host');
		assert.equal(manifest.architecture, process.arch, 'Release architecture differs from the verifier host');
		const actual = (await releaseInventory(this.directory)).filter(file => file.path !== 'build-manifest.json');
		assert.deepEqual(actual, manifest.files, 'Release inventory differs: files missing, added or modified');
		return manifest;
	}

	async verifyNative() {
		await this.verifyFiles();
		const addon = createRequire(import.meta.url)(join(this.directory, 'app/native/vnilink.node'));
		const engine = await addon.openTemporaryEngine();
		try {
			const reply = JSON.parse(await engine.execute(JSON.stringify({ type: 'snapshot' })));
			assert.equal(reply.type, 'snapshot');
			assert.match(reply.snapshot.endpoint_id, /^[a-f0-9]{64}$/);
			assert.equal(reply.snapshot.devices.length, 0, 'Temporary release probe unexpectedly has paired devices');
		} finally {
			await engine.execute(JSON.stringify({ type: 'shutdown' }));
		}
		console.log('Release payload verified; staged addon opened and shut down a temporary engine under Node. Packaged Electron/OS behaviour remains a later check.');
	}
}
