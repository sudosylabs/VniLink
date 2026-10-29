import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';
import { build } from 'esbuild';
import { trayFileName, trayStates, trayVariants } from '../../build/lib/trayAssets.ts';

const root = fileURLToPath(new URL('../../', import.meta.url));

const electron = `import { EventEmitter } from 'node:events';
export const calls = { quit: 0, trays: [] };
export const app = { quit() { calls.quit++; } };
export const Menu = { buildFromTemplate: template => ({ template }) };
export const nativeImage = { createFromPath: path => ({ path }) };
export const nativeTheme = Object.assign(new EventEmitter(), { shouldUseDarkColorsForSystemIntegratedUI: true });
export class Tray extends EventEmitter {
	constructor(image) { super(); this.image = image; this.destroyed = false; this.menus = []; this.images = [image]; calls.trays.push(this); }
	setImage(image) { this.image = image; this.images.push(image); }
	setToolTip(tooltip) { this.tooltip = tooltip; }
	setContextMenu(menu) { this.menu = menu; this.menus.push(menu); }
	destroy() { this.destroyed = true; }
	isDestroyed() { return this.destroyed; }
}
export class MessageChannelMain { }
export const utilityProcess = {};
export const ipcMain = {};
export default { app, ipcMain };`;

const bundle = await build({
	stdin: {
		contents: `export { TrayMainService, trayState, trayMenu, trayImageName, trayStatusLine } from './src/platform/tray/electron-main/trayMainService.js';
			export { traySupported } from './src/platform/tray/electron-main/traySupport.js';
			export { Emitter } from './src/base/common/event.js';
			export { calls, nativeTheme } from 'electron';`,
		loader: 'ts', resolveDir: root,
	},
	bundle: true, write: false, platform: 'node', format: 'esm', target: 'node22', tsconfig: `${root}tsconfig.json`,
	// The service resolves its artwork beside the CommonJS main bundle.
	banner: { js: `const __dirname = ${JSON.stringify(`${root}out`)};` },
	plugins: [{
		name: 'electron-fixture', setup(builder) {
			builder.onResolve({ filter: /^electron$/ }, () => ({ path: 'electron', namespace: 'fixture' }));
			builder.onLoad({ filter: /.*/, namespace: 'fixture' }, () => ({ contents: electron, loader: 'js', resolveDir: root }));
		}
	}],
});

const { TrayMainService, trayState, trayMenu, trayImageName, trayStatusLine, traySupported, Emitter, calls, nativeTheme } = await import(`data:text/javascript;base64,${Buffer.from(bundle.outputFiles[0].text).toString('base64')}`);

const ready = { available: true, accepting: true, hosting: true, sessions: [] };

const connected = { ...ready, sessions: [{ id: '7', name: 'Work laptop', phase: 'Active' }, { id: '9', name: 'host-b', phase: 'Negotiating' }] };

const labels = menu => menu.filter(entry => 'label' in entry).map(entry => entry.label);

test('the tray state and status line say whether devices can reach this computer and who has', () => {
	assert.equal(trayState(ready), 'idle');
	assert.equal(trayState(connected), 'active');
	assert.equal(trayState({ ...ready, accepting: false, hosting: false }), 'paused');
	assert.equal(trayState({ ...ready, hosting: false }), 'paused', 'Not hosting yet must not look ready');
	assert.equal(trayState({ ...ready, available: false, hosting: false }), 'paused');
	assert.equal(trayStatusLine(ready), 'Ready for paired devices');
	assert.equal(trayStatusLine(connected), '2 devices connected');
	assert.equal(trayStatusLine({ ...connected, sessions: connected.sessions.slice(0, 1) }), '1 device connected');
	assert.equal(trayStatusLine({ ...ready, accepting: false }), 'Not accepting connections');
	assert.equal(trayStatusLine({ ...ready, available: false }), 'Connection engine unavailable');
});

test('the tray menu offers a disconnect per device, the opposite of the current accept choice, and always Open and Quit', () => {
	const menu = trayMenu(connected);
	assert.deepEqual(labels(menu), ['2 devices connected', 'Connected to this computer', 'Disconnect Work laptop', 'Disconnect host-b (connecting)', 'Stop Accepting Connections', 'Open VniLink', 'Quit VniLink']);
	assert.deepEqual(menu.filter(entry => entry.action?.kind === 'disconnect').map(entry => entry.action.sessionId), ['7', '9']);
	assert.equal(menu[0].enabled, false, 'The status line is not an action');
	assert.deepEqual(labels(trayMenu({ ...ready, accepting: false, hosting: false })), ['Not accepting connections', 'Accept Connections', 'Open VniLink', 'Quit VniLink']);
	assert.deepEqual(labels(trayMenu({ available: false, accepting: true, hosting: false, sessions: [] })), ['Connection engine unavailable', 'Open VniLink', 'Quit VniLink'], 'Nothing to change while the engine is unavailable');
});

test('tray artwork follows the platform and, on Windows, the taskbar theme', () => {
	assert.equal(trayImageName('active', 'darwin', true), 'activeTemplate.png');
	assert.equal(trayImageName('idle', 'win32', true), 'idle-dark.png');
	assert.equal(trayImageName('idle', 'win32', false), 'idle-light.png');
	assert.equal(trayImageName('paused', 'linux', false), 'paused-dark.png', 'Linux panels are assumed dark');
});

function pngSize(bytes) {
	assert.equal(bytes.subarray(1, 4).toString('latin1'), 'PNG');
	return [bytes.readUInt32BE(16), bytes.readUInt32BE(20)];
}

test('every tray image the service can request exists at each scale Electron loads', async () => {
	for (const variant of trayVariants) {
		for (const state of trayStates) {
			for (const scale of variant.scales) {
				const pixels = Math.round(variant.size * scale);
				const name = trayFileName(variant, state, scale);
				assert.deepEqual(pngSize(await readFile(`${root}assets/tray/${name}`)), [pixels, pixels], name);
			}
		}
	}
	const rendered = new Set(trayVariants.flatMap(variant => trayStates.map(state => trayFileName(variant, state, 1))));
	for (const platform of ['darwin', 'win32', 'linux']) {
		for (const state of trayStates) {
			for (const dark of [false, true]) {
				assert(rendered.has(trayImageName(state, platform, dark)), `${platform} ${state} has no rendered image`);
			}
		}
	}
});

function hosting(status) {
	const changes = new Emitter();
	const requests = [];
	return {
		status, changes, requests,
		onDidChangeStatus: changes.event,
		async setAccepting(value) {
			requests.push(['accept', value]);
		},
		async disconnect(id) {
			requests.push(['disconnect', id]);
		},
		set(next) {
			this.status = next;
			changes.fire(next);
		},
	};
}

test('the tray service shows status changes, runs menu actions and is removed on dispose', () => {
	const source = hosting(ready);
	const service = new TrayMainService(source, { error() { } });
	let opened = 0;
	service.onDidRequestWindow(() => opened++);
	service.create();
	service.create();
	assert.equal(calls.trays.length, 1, 'One tray per application');
	const tray = calls.trays[0];
	assert.match(tray.image.path, /assets[\\/]tray[\\/]idle(Template|-dark|-light)\.png$/);
	assert.equal(tray.tooltip, 'VniLink – Ready for paired devices');
	source.set(connected);
	assert.match(tray.image.path, /active/);
	const click = label => tray.menu.template.find(item => item.label === label).click();
	click('Disconnect Work laptop');
	click('Stop Accepting Connections');
	click('Open VniLink');
	click('Quit VniLink');
	assert.deepEqual(source.requests, [['disconnect', '7'], ['accept', false]]);
	assert.equal(opened, 1);
	assert.equal(calls.quit, 1);
	const menus = tray.menus.length;
	const images = tray.images.length;
	source.set({ ...connected });
	nativeTheme.emit('updated');
	assert.equal(tray.menus.length, menus, 'An unchanged menu must not be rebuilt');
	assert.equal(tray.images.length, images, 'An unchanged image must not be reloaded');
	if (process.platform !== 'darwin') {
		tray.emit('click');
		assert.equal(opened, 2, 'Clicking the icon opens the window on Windows and Linux');
	}
	service.dispose();
	assert.equal(tray.destroyed, true);
	assert.equal(source.changes.hasListeners(), false, 'Disposed tray kept listening to hosting');
	assert.equal(nativeTheme.listenerCount('updated'), 0, 'Disposed tray kept listening to the theme');
});

test('Linux offers a tray only while a StatusNotifierItem watcher owns its bus name', async () => {
	assert.equal(await traySupported(async () => {
		throw new Error('unused');
	}, false), true, 'macOS and Windows always show tray icons');
	assert.equal(await traySupported(async () => ({ stdout: '(true,)\n' }), true), true);
	assert.equal(await traySupported(async () => ({ stdout: '(false,)\n' }), true), false);
	assert.equal(await traySupported(async () => {
		throw Object.assign(new Error('spawn gdbus ENOENT'), { code: 'ENOENT' });
	}, true), false, 'No gdbus means no way to know, so no tray');
});
