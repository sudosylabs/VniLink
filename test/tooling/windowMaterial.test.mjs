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
	for (const [platform, release, expected] of [['darwin', '25.0.0', 'vibrancy'], ['win32', '10.0.22621', 'mica'], ['win32', '10.0.22000', 'none'], ['win32', '10.0.19045', 'none'], ['linux', '6.8.0', 'none'], ['darwin', 'invalid', 'none']]) {
		assert.equal(resolveWindowMaterial(platform, release, false, false), expected);
		assert.equal(resolveWindowMaterial(platform, release, true, false), 'none');
		assert.equal(resolveWindowMaterial(platform, release, false, true), 'none');
	}
});
function windowFixture() {
	const calls = [];
	return { calls, isDestroyed: () => false, setBackgroundMaterial: value => calls.push(['material', value]), setVibrancy: value => calls.push(['vibrancy', value]), setBackgroundColor: value => calls.push(['background', value]) };
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
test('macOS uses sidebar vibrancy and a failed native backdrop stays opaque', () => {
	const window = windowFixture();
	const material = new WindowMaterial(window, 'darwin', '25.0.0');
	assert.equal(material.material, 'vibrancy');
	assert.deepEqual(window.calls.slice(-2), [['vibrancy', 'sidebar'], ['background', '#00000000']]);
	material.dispose();
	window.setVibrancy = () => {
		throw new Error('Native composition unavailable');
	};
	const fallback = new WindowMaterial(window, 'darwin', '25.0.0');
	assert.equal(fallback.material, 'none');
	assert.deepEqual(window.calls.at(-1), ['background', '#fafafa']);
	fallback.dispose();
});

test('saved native-background preference disables and restores the platform material', () => {
	const window = windowFixture();
	const material = new WindowMaterial(window, 'darwin', '25.0.0');
	try {
		material.setEnabled(false);
		assert.equal(material.material, 'none');
		assert.deepEqual(window.calls.slice(-2), [['vibrancy', null], ['background', '#fafafa']]);
		material.setEnabled(true);
		assert.equal(material.material, 'vibrancy');
	} finally {
		material.dispose();
	}
});
