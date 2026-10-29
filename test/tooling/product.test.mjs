import { test } from 'node:test';
import assert from 'node:assert/strict';
import { fileURLToPath } from 'node:url';
import { loadProduct, validateProduct } from '../../build/lib/product.ts';

const product = await loadProduct(fileURLToPath(new URL('../../', import.meta.url)));

test('product identity has existing artwork and consistent native targets', () => {
	assert.equal(validateProduct(product), product);
	assert.equal(Object.values(product.platforms).flatMap(platform => platform.targets).length, 4);
});

test('product rejects a mismatched OS/architecture/native ABI and duplicate targets', () => {
	const wrongAbi = structuredClone(product);
	wrongAbi.platforms.win32.targets[0].rustTarget = 'x86_64-pc-windows-msvc';
	assert.throws(() => validateProduct(wrongAbi), /architecture\/Rust target pair/);
	const duplicate = structuredClone(product);
	duplicate.platforms.darwin.targets.push(duplicate.platforms.darwin.targets[0]);
	assert.throws(() => validateProduct(duplicate), /duplicate darwin architecture/);
});

test('product rejects escaped artwork paths and inconsistent desktop identity', () => {
	const escaped = structuredClone(product);
	escaped.platforms.linux.icon = 'assets/../private/icon.png';
	assert.throws(() => validateProduct(escaped), /icon path/);
	const desktop = structuredClone(product);
	desktop.platforms.linux.desktopFileName = 'other.desktop';
	assert.throws(() => validateProduct(desktop), /desktop identity/);
});

test('product rejects an OS floor below the native API baseline', () => {
	const oldMac = structuredClone(product);
	oldMac.platforms.darwin.minimumSystemVersion = '13.0';
	assert.throws(() => validateProduct(oldMac), /macOS floor/);
});

test('product rejects malformed metadata without treating it as a valid target', () => {
	for (const change of [
		value => {
			value.platforms.win32.targets = [null];
		},
		value => {
			value.platforms.linux.intendedDistributions = [{ id: 'ubuntu', minimumVersion: 'unknown' }];
		},
		value => {
			value.appId = 'VniLink';
		},
		value => {
			value.platforms.linux.signing = 'developer-id-and-notarization';
		},
	]) {
		const candidate = structuredClone(product);
		change(candidate);
		assert.throws(() => validateProduct(candidate), /Invalid product metadata/);
	}
});
