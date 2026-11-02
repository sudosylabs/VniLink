// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'assert';
import { suite, test } from 'node:test';
import { fileURLToPath } from 'node:url';
import { loadProduct, validateProduct, type Product } from '../product.ts';

const product = await loadProduct(fileURLToPath(new URL('../../../', import.meta.url)));

const { darwin, win32, linux } = product.platforms;

/** The product with one platform's metadata replaced. */
function withPlatform(platforms: Partial<Record<keyof Product['platforms'], object>>): object {
	return { ...product, platforms: { ...product.platforms, ...platforms } };
}

suite('product', () => {
	test('accepts the repository metadata, whose artwork exists', () => {
		assert.strictEqual(validateProduct(product), product);
	});
	test('declares four native targets', () => {
		assert.strictEqual(Object.values(product.platforms).flatMap(platform => platform.targets).length, 4);
	});
	test('rejects a Rust target of another native ABI', () => {
		const wrongAbi = withPlatform({ win32: { ...win32, targets: [{ ...win32.targets[0], rustTarget: 'x86_64-pc-windows-msvc' }, ...win32.targets.slice(1)] } });
		assert.throws(() => validateProduct(wrongAbi), /architecture\/Rust target pair/);
	});
	test('rejects a duplicate architecture', () => {
		const duplicate = withPlatform({ darwin: { ...darwin, targets: [...darwin.targets, darwin.targets[0]] } });
		assert.throws(() => validateProduct(duplicate), /duplicate darwin architecture/);
	});
	test('rejects an artwork path that escapes the assets', () => {
		const escaped = withPlatform({ linux: { ...linux, icon: 'assets/../private/icon.png' } });
		assert.throws(() => validateProduct(escaped), /icon path/);
	});
	test('rejects a desktop file that does not match the application id', () => {
		const desktop = withPlatform({ linux: { ...linux, desktopFileName: 'other.desktop' } });
		assert.throws(() => validateProduct(desktop), /desktop identity/);
	});
	test('rejects a macOS floor below the native API baseline', () => {
		const oldMac = withPlatform({ darwin: { ...darwin, minimumSystemVersion: '13.0' } });
		assert.throws(() => validateProduct(oldMac), /macOS floor/);
	});
	const malformed: readonly (readonly [string, object])[] = [
		['a null target', withPlatform({ win32: { ...win32, targets: [null] } })],
		['a distribution without a version number', withPlatform({ linux: { ...linux, intendedDistributions: [{ id: 'ubuntu', minimumVersion: 'unknown' }] } })],
		['an application id that is not reverse-DNS', { ...product, appId: 'VniLink' }],
		['a signing mode of another OS', withPlatform({ linux: { ...linux, signing: 'developer-id-and-notarization' } })],
	];
	for (const [name, candidate] of malformed) {
		test(`rejects ${name} as invalid metadata`, () => {
			assert.throws(() => validateProduct(candidate), /Invalid product metadata/);
		});
	}
});
