// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'assert';
import { suite, test } from 'node:test';
import { fileURLToPath } from 'node:url';
import { assemblyEnvironment, packageConfiguration, packagedLayout, type BuilderConfiguration } from '../package.ts';
import { readProduct, type ProductPlatform } from '../product.ts';

const product = await readProduct(fileURLToPath(new URL('../../../', import.meta.url)));

const platforms: readonly ProductPlatform[] = ['darwin', 'win32', 'linux'];

function configuration(platform: ProductPlatform): BuilderConfiguration {
	return packageConfiguration('/repo', '/stage', '/output', product, { platform, electronVersion: '44.7.0' });
}

/** Where each extra resource is placed in the packaged resources. */
function resourceDestinations(config: BuilderConfiguration): unknown[] {
	assert.ok(Array.isArray(config.extraResources));
	return config.extraResources.map((resource: unknown) => typeof resource === 'object' && resource !== null && 'to' in resource ? resource.to : undefined);
}

function packagedPath(path: string): string {
	return path.split('\\').join('/');
}

suite('packageConfiguration', () => {
	for (const platform of platforms) {
		test(`keeps the native module and the engine outside the ASAR on ${platform}`, () => {
			assert.deepStrictEqual(configuration(platform).asarUnpack, ['native/**', 'engine.cjs']);
		});
		test(`packages the product's application id on ${platform}`, () => {
			assert.strictEqual(configuration(platform).appId, product.appId);
		});
		test(`packages the given Electron version on ${platform}`, () => {
			assert.strictEqual(configuration(platform).electronVersion, '44.7.0');
		});
		test(`does not rebuild dependencies on ${platform}`, () => {
			assert.strictEqual(configuration(platform).npmRebuild, false);
		});
		test(`associates invitation files with the application on ${platform}`, () => {
			assert.deepStrictEqual(configuration(platform).fileAssociations, [
				{ ext: product.invitation.extension, name: product.invitation.name, mimeType: product.invitation.mimeType, role: 'Viewer' },
			]);
		});
		test(`publishes nothing on ${platform}`, () => {
			assert.strictEqual(configuration(platform).publish, null);
		});
		test(`signs and notarizes nothing on ${platform}`, () => {
			const config = configuration(platform);
			assert.strictEqual(config.mac.identity, null);
			assert.strictEqual(config.win.signExecutable, false);
			assert.strictEqual(config.mac.notarize, false);
		});
		test(`ships the relink inputs as a resource, not in the application, on ${platform}`, () => {
			const config = configuration(platform);
			assert.ok(Array.isArray(config.files) && config.files.includes('!relink/**'));
			assert.ok(resourceDestinations(config).includes('relink'));
		});
		test(`${platform === 'linux' ? 'ships' : 'does not ship'} the icon as a resource on ${platform}`, () => {
			assert.strictEqual(resourceDestinations(configuration(platform)).includes('app-icon.png'), platform === 'linux');
		});
		test(`places the native module in app.asar.unpacked on ${platform}`, () => {
			assert.ok(packagedPath(packagedLayout('/application', platform, product).native).endsWith('/app.asar.unpacked/native/vnilink.node'));
		});
		test(`places the executable where ${platform} launches it`, () => {
			const executable = platform === 'darwin' ? '/VniLink.app/Contents/MacOS/VniLink' : platform === 'win32' ? '/vnilink.exe' : '/vnilink';
			assert.ok(packagedPath(packagedLayout('/application', platform, product).executable).endsWith(executable));
		});
	}
	test('an Authenticode release has the packager sign the Windows executables, and nothing else', () => {
		const config = packageConfiguration('/repo', '/stage', '/output', product, { platform: 'win32', electronVersion: '44.7.0' }, 'authenticode');
		assert.deepStrictEqual([config.win.signExecutable, config.mac.identity, config.mac.notarize], [true, null, false]);
	});
	test('unsigned assembly neither discovers signing credentials nor inherits Node launch flags', () => {
		assert.deepStrictEqual(assemblyEnvironment({ PATH: '/tools', CSC_LINK: 'secret', CSC_KEY_PASSWORD: 'secret', WIN_CSC_LINK: 'secret', APPLE_ID: 'secret', CSC_IDENTITY_AUTO_DISCOVERY: 'true', ELECTRON_RUN_AS_NODE: '1', NODE_OPTIONS: '--inspect' }), { PATH: '/tools', CSC_IDENTITY_AUTO_DISCOVERY: 'false' });
	});
});
