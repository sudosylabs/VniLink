// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'node:assert/strict';
import test from 'node:test';
import { build } from 'esbuild';

const output = await build({
	stdin: { contents: `export { WindowMaterial } from './src/platform/windows/electron-main/windowMaterial.js'; export { resolveWindowMaterial } from './src/platform/window/common/windowMaterial.js'; export { nativeTheme } from 'electron';`, resolveDir: process.cwd(), loader: 'ts' }, bundle: true, write: false, platform: 'node', format: 'esm', plugins: [{
		name: 'theme-fixture', setup(builder) {
			builder.onResolve({ filter: /^electron$/ }, () => ({ path: 'electron', namespace: 'fixture' }));
			builder.onLoad({ filter: /.*/, namespace: 'fixture' }, () => ({ contents: `import { EventEmitter } from 'node:events'; export const nativeTheme = Object.assign(new EventEmitter(), { shouldUseDarkColors: false, prefersReducedTransparency: false, shouldUseHighContrastColors: false });`, loader: 'js' }));
		}
	}]
});

const { WindowMaterial, resolveWindowMaterial, nativeTheme } = await import(`data:text/javascript;base64,${Buffer.from(output.outputFiles[0].text).toString('base64')}`);

test('native backdrop policy gates OS versions and accessibility preferences', () => {
	for (const [platform, release, expected] of [['darwin', '27.0.0', 'glass'], ['darwin', '25.0.0', 'glass'], ['darwin', '24.6.0', 'vibrancy'], ['darwin', '23.0.0', 'vibrancy'], ['win32', '10.0.22621', 'mica'], ['win32', '10.0.22000', 'none'], ['win32', '10.0.19045', 'none'], ['linux', '6.8.0', 'none'], ['darwin', 'invalid', 'none']]) {
		assert.equal(resolveWindowMaterial(platform, release, false, false), expected);
		assert.equal(resolveWindowMaterial(platform, release, true, false), 'none');
		assert.equal(resolveWindowMaterial(platform, release, false, true), 'none');
	}
});

const handle = Buffer.alloc(8, 1);

function windowFixture() {
	const calls = [];
	return { calls, isDestroyed: () => false, getNativeWindowHandle: () => handle, setBackgroundMaterial: value => calls.push(['material', value]), setVibrancy: value => calls.push(['vibrancy', value]), setBackgroundColor: value => calls.push(['background', value]) };
}

/** Stands in for @vnilink/elg's owner, recording what the window asks of it. */
function glassFixture({ supported = true, id = 4 } = {}) {
	const calls = [];
	return {
		calls,
		owner: {
			isGlassSupported: () => supported,
			addView: (view, options) => {
				calls.push(['add', view === handle, options]);
				return id;
			},
			removeView: view => {
				calls.push(['remove', view]);
				return true;
			},
			dispose: () => calls.push(['dispose']),
		},
	};
}

test('Mica follows transparency and theme changes and disposes its listener', () => {
	const window = windowFixture();
	const material = new WindowMaterial(window, 'win32', '10.0.22631');
	try {
		assert.equal(material.material, 'mica');
		assert.deepEqual(window.calls.slice(-2), [['material', 'mica'], ['background', '#00000000']]);
		nativeTheme.prefersReducedTransparency = true;
		nativeTheme.shouldUseDarkColors = true;
		nativeTheme.emit('updated');
		assert.equal(material.material, 'none');
		assert.deepEqual(window.calls.slice(-2), [['material', 'none'], ['background', '#202124']]);
		nativeTheme.prefersReducedTransparency = false;
		nativeTheme.emit('updated');
		assert.equal(material.material, 'mica');
	} finally {
		material.dispose();
		nativeTheme.prefersReducedTransparency = false;
		nativeTheme.shouldUseDarkColors = false;
	}
	const count = window.calls.length;
	nativeTheme.emit('updated');
	assert.equal(window.calls.length, count);
	assert.equal(nativeTheme.listenerCount('updated'), 0);
});

test('macOS 26 attaches Liquid Glass, re-attaches after accessibility changes and releases it', () => {
	const window = windowFixture();
	const glass = glassFixture();
	let created = 0;
	const material = new WindowMaterial(window, 'darwin', '25.0.0', () => {
		created++;
		return glass.owner;
	});
	try {
		assert.equal(material.material, 'glass');
		// Glass sits on a window-coloured backing in the theme's sidebar colour, whatever the wallpaper.
		assert.deepEqual(glass.calls, [['add', true, { opaque: true, tintColor: '#F1F1F3CC' }]]);
		assert.deepEqual(window.calls.slice(-2), [['vibrancy', null], ['background', '#00000000']]);
		// Reduce Transparency removes system glass; the window goes opaque.
		nativeTheme.prefersReducedTransparency = true;
		nativeTheme.emit('updated');
		assert.equal(material.material, 'none');
		assert.deepEqual(glass.calls.at(-1), ['remove', 4]);
		assert.deepEqual(window.calls.at(-1), ['background', '#fafafa']);
		// Turning it off does not restore glass by itself, so the window attaches again.
		nativeTheme.prefersReducedTransparency = false;
		nativeTheme.emit('updated');
		assert.equal(material.material, 'glass');
		assert.deepEqual(glass.calls.at(-1), ['add', true, { opaque: true, tintColor: '#F1F1F3CC' }]);
		// A theme change re-tints the glass with that theme's sidebar colour.
		nativeTheme.shouldUseDarkColors = true;
		nativeTheme.emit('updated');
		assert.deepEqual(glass.calls.at(-1), ['add', true, { opaque: true, tintColor: '#26272BB8' }]);
		assert.equal(created, 1, 'A window made a second owner; the first would remove its glass when collected');
	} finally {
		material.dispose();
		nativeTheme.prefersReducedTransparency = false;
		nativeTheme.shouldUseDarkColors = false;
	}
	assert.deepEqual(glass.calls.at(-1), ['dispose']);
});

test('macOS falls back to vibrancy when Liquid Glass is missing or refused', () => {
	for (const factory of [() => undefined, () => glassFixture({ supported: false }).owner, () => glassFixture({ id: -1 }).owner]) {
		const window = windowFixture();
		const material = new WindowMaterial(window, 'darwin', '25.0.0', factory);
		assert.equal(material.material, 'vibrancy');
		assert.deepEqual(window.calls.slice(-2), [['vibrancy', 'sidebar'], ['background', '#00000000']]);
		material.dispose();
	}
});

test('macOS uses sidebar vibrancy and a failed native backdrop stays opaque', () => {
	const window = windowFixture();
	const material = new WindowMaterial(window, 'darwin', '24.6.0');
	assert.equal(material.material, 'vibrancy');
	assert.deepEqual(window.calls.slice(-2), [['vibrancy', 'sidebar'], ['background', '#00000000']]);
	material.dispose();
	window.setVibrancy = () => {
		throw new Error('Native composition unavailable');
	};
	const fallback = new WindowMaterial(window, 'darwin', '24.6.0');
	assert.equal(fallback.material, 'none');
	assert.deepEqual(window.calls.at(-1), ['background', '#fafafa']);
	fallback.dispose();
});
