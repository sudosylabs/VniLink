import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';
import { assemblyEnvironment, packageConfiguration, packagedLayout } from '../../build/lib/package.ts';

const product = JSON.parse(await readFile(new URL('../../product.json', import.meta.url), 'utf8'));

test('directory packaging keeps the engine and native module adjacent outside ASAR on every host', () => {
	for (const platform of ['darwin', 'win32', 'linux']) {
		const config = packageConfiguration('/repo', '/stage', '/output', product, { platform, electronVersion: '44.7.0' });
		assert.deepEqual(config.asarUnpack, ['native/**', 'engine.cjs']);
		assert.equal(config.appId, 'fr.sudosy.vnilink');
		assert.equal(config.electronVersion, '44.7.0');
		assert.equal(config.npmRebuild, false);
		assert.equal(config.publish, null);
		assert.equal(config.mac.identity, null);
		assert.equal(config.win.signExecutable, false);
		assert.equal(config.mac.notarize, false);
		assert(config.files.includes('!relink/**'));
		assert(config.extraResources.some(resource => resource.to === 'relink'));
		assert.equal(config.extraResources.some(resource => resource.to === 'app-icon.png'), platform === 'linux');
		const layout = packagedLayout('/application', platform, product);
		assert(layout.native.split('\\').join('/').endsWith('/app.asar.unpacked/native/vnilink.node'));
		assert(layout.executable.split('\\').join('/').endsWith(platform === 'darwin' ? '/VniLink.app/Contents/MacOS/VniLink' : platform === 'win32' ? '/vnilink.exe' : '/vnilink'));
	}
});

test('unsigned assembly cannot discover supplied signing credentials or inherit Node launch flags', () => {
	assert.deepEqual(assemblyEnvironment({ PATH: '/tools', CSC_LINK: 'secret', CSC_KEY_PASSWORD: 'secret', WIN_CSC_LINK: 'secret', APPLE_ID: 'secret', CSC_IDENTITY_AUTO_DISCOVERY: 'true', ELECTRON_RUN_AS_NODE: '1', NODE_OPTIONS: '--inspect' }), { PATH: '/tools', CSC_IDENTITY_AUTO_DISCOVERY: 'false' });
});
