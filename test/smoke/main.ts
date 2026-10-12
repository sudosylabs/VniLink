import assert from 'node:assert/strict';
import { getMacOSWindowControlsPosition, getNativeTitlebarHeight } from '../../src/platform/window/common/window.js';
import { isEngineReply, record } from '../../src/platform/engine/common/engine.js';
import type { BrowserWindow } from 'electron';
import { app, net, nativeTheme } from 'electron';
import { mkdtempSync, rmSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
import { tmpdir, release } from 'node:os';
import { isMacintosh } from '../../src/base/common/platform.js';
import { isAppInfo } from '../../src/platform/ipc/common/desktopApi.js';
import { startApplication } from '../../src/app/electron-main/application.js';

const profile = mkdtempSync(join(tmpdir(), 'vnilink-electron-smoke-'));
app.setPath('userData', profile);
app.on('web-contents-created', (_event, contents) => {
	contents.on('console-message', details => {
		if (details.level === 'error') {
			console.error(details.message);
		}
	});
});
const timeout = setTimeout(() => {
	console.error('Smoke test timed out');
	app.exit(1);
}, 60_000);
app.on('quit', () => {
	clearTimeout(timeout);
	rmSync(profile, { recursive: true, force: true });
});
async function verify(window: BrowserWindow): Promise<void> {
	await window.webContents.executeJavaScript(`new Promise((resolve, reject) => {
    if (document.documentElement.dataset.workbenchReady === 'true') return resolve(true);
    const timeout = setTimeout(() => { observer.disconnect(); reject(new Error('Workbench did not render')); }, 5000);
    const observer = new MutationObserver(() => {
      if (document.documentElement.dataset.workbenchReady === 'true') { clearTimeout(timeout); observer.disconnect(); resolve(true); }
    });
    observer.observe(document.documentElement, { attributes: true, childList: true, subtree: true });
  })`);
	const result: unknown = await window.webContents.executeJavaScript(`(async () => {
    const info = await window.vnilink.getAppInfo();
    return { info, nodeExposed: typeof require !== 'undefined' || typeof process !== 'undefined',
      title: document.querySelector('h1')?.textContent, api: Object.keys(window.vnilink).sort() };
  })()`);
	if (
		typeof result !== 'object' ||
		result === null ||
		!('nodeExposed' in result) ||
		result.nodeExposed !== false ||
		!('info' in result) ||
		!isAppInfo(result.info) ||
		result.info.version !== app.getVersion() ||
		!('title' in result) ||
		result.title !== 'Your computers' ||
		!('api' in result) ||
		JSON.stringify(result.api) !==
		JSON.stringify(['decryptText', 'encryptText', 'engineCommand', 'getAppInfo', 'getKeyStorageProvider', 'isEncryptionAvailable', 'nativeHostCommand', 'onWindowStateChanged', 'openDataChannel'])
	) {
		throw new Error(`Unexpected renderer state: ${JSON.stringify(result)}`);
	}
	const iconTests = readFileSync(join(__dirname, 'icon-tests.js'), 'utf8');
	const iconsVerified: unknown = await window.webContents.executeJavaScript(`${iconTests}\niconTests.verifyIcons()`);
	if (iconsVerified !== true) {
		throw new Error('Icon helper verification failed');
	}
	const tabTests = readFileSync(join(__dirname, 'tab-tests.js'), 'utf8');
	const tabsVerified: unknown = await window.webContents.executeJavaScript(`${tabTests}\ntabTests.verifyTabs()`);
	assert.equal(tabsVerified, true);
	const dataChannelTests = readFileSync(join(__dirname, 'data-channel-tests.js'), 'utf8');
	assert.equal(await window.webContents.executeJavaScript(`${dataChannelTests}\ndataChannelTests.verifyDataChannels()`), true);
	const engine: unknown = await window.webContents.executeJavaScript(`(async () => {
    const snapshot = await window.vnilink.engineCommand({ type: 'snapshot' });
    const invitation = await window.vnilink.engineCommand({ type: 'createInvitation', ttlSeconds: 60 });
    let invalidRejected = false;
    try { await window.vnilink.engineCommand({ type: 'shutdown' }); } catch { invalidRejected = true; }
    let sessionError = '';
    try { await window.vnilink.engineCommand({ type: 'disconnect' }); } catch (error) { sessionError = String(error); }
    const afterError = await window.vnilink.engineCommand({ type: 'snapshot' });
    const encryptionAvailable = await window.vnilink.isEncryptionAvailable();
    return { snapshot, invitation, invalidRejected, sessionError, afterError, encryptionAvailable };
  })()`);
	if (
		!record(engine) ||
		!isEngineReply(engine.snapshot) ||
		engine.snapshot.type !== 'snapshot' ||
		engine.snapshot.snapshot.endpoint_id.length !== 64 ||
		engine.snapshot.snapshot.hosting ||
		!isEngineReply(engine.invitation) ||
		engine.invitation.type !== 'invitation' ||
		!engine.invitation.token ||
		engine.invalidRejected !== true ||
		typeof engine.sessionError !== 'string' ||
		!engine.sessionError.includes('NoSession') ||
		!isEngineReply(engine.afterError) ||
		engine.afterError.type !== 'snapshot' ||
		typeof engine.encryptionAvailable !== 'boolean'
	) {
		throw new Error('Native control bridge failed');
	}
	let rejected = false;
	try {
		await window.webContents.executeJavaScript(`window.vnilink.nativeHostCommand('invalid', [1])`);
	} catch {
		rejected = true;
	}
	if (!rejected) {
		throw new Error('Invalid window action accepted');
	}
	if ((await net.fetch('vnilink://app/main.cjs')).status !== 404) {
		throw new Error('Unlisted asset accessible');
	}
	await verifyTitlebar(window);
	console.log(
		'PASS: icon variants, accessibility, theming and buttons verified; native titlebar, theme, window state and controls verified; application loaded, typed bridge works, renderer isolated, invalid action rejected, native Rust state and invitations work, privileged shutdown rejected.',
	);
	app.quit();
}
async function verifyTitlebar(window: BrowserWindow): Promise<void> {
	const height = Math.max(35, getNativeTitlebarHeight(release()));
	const presentation: unknown = await window.webContents.executeJavaScript(
		`({ height: document.querySelector('.part.titlebar').getBoundingClientRect().height, drag: getComputedStyle(document.querySelector('.titlebar-drag-region')).getPropertyValue('-webkit-app-region'), unwanted: document.querySelectorAll('.window-controls, .titlebar-center, .menubar').length })`,
	);
	assert.deepEqual(presentation, { height, drag: 'drag', unwanted: 0 });
	if (isMacintosh) {
		assert.deepEqual(window.getWindowButtonPosition(), getMacOSWindowControlsPosition(height, release()));
	}
	for (const zoom of [1.25, 1]) {
		window.webContents.setZoomFactor(zoom);
		const expected = height / zoom;
		const zoomed: unknown = await window.webContents.executeJavaScript(`new Promise((resolve, reject) => { const start = performance.now(); function check() { const height = document.querySelector('.part.titlebar').getBoundingClientRect().height; if (Math.abs(height - ${String(expected)}) < 0.1) { resolve(true); } else if (performance.now() - start > 5000) { reject(new Error('Titlebar zoom layout did not update')); } else { requestAnimationFrame(check); } } check(); })`);
		assert.equal(zoomed, true);
	}
	const rejected: unknown = await window.webContents.executeJavaScript(
		`(async () => { const { windowId } = await window.vnilink.getAppInfo(); let rejected = 0; for (const [method, args] of [['getWindowState', [windowId + 1000]], ['updateWindowControls', [windowId, { height: -1 }]], ['focusWindow', [windowId, { targetWindowId: windowId + 1000 }]], ['updateWindowControls', [windowId, { arbitrary: true }]]]) { try { await window.vnilink.nativeHostCommand(method, args); } catch { rejected++; } } return rejected; })()`,
	);
	assert.equal(rejected, 4);
	// Exercise the event transport and unsubscribe using a real Electron window state change.
	await window.webContents.executeJavaScript(
		`globalThis.__smokeWindowState = new Promise((resolve, reject) => { const timeout = setTimeout(() => { stop(); reject(new Error('Window state event not received')); }, 5000); const stop = window.vnilink.onWindowStateChanged(state => { if (state.alwaysOnTop) { clearTimeout(timeout); stop(); resolve(state); } }); }); true`,
	);
	window.setAlwaysOnTop(true);
	const received: unknown = await window.webContents.executeJavaScript(`globalThis.__smokeWindowState`);
	assert.ok(record(received) && received.windowId === window.id && received.alwaysOnTop === true);
	window.setAlwaysOnTop(false);
	const originalTheme = nativeTheme.themeSource;
	try {
		for (const [theme, background] of [
			['dark', 'rgb(44, 45, 49)'],
			['light', 'rgb(236, 236, 239)'],
		] as const) {
			nativeTheme.themeSource = theme;
			const themed: unknown = await window.webContents.executeJavaScript(
				`new Promise((resolve, reject) => { const start = performance.now(); function check() { if (getComputedStyle(document.querySelector('.part.titlebar')).backgroundColor === ${JSON.stringify(background)}) { resolve(true); } else if (performance.now() - start > 5000) { reject(new Error('Titlebar theme did not update')); } else { requestAnimationFrame(check); } } check(); })`,
			);
			assert.equal(themed, true);
		}
	} finally {
		nativeTheme.themeSource = originalTheme;
	}
	const call = async (method: string, args = ''): Promise<unknown> =>
		await window.webContents.executeJavaScript(
			`window.vnilink.getAppInfo().then(info => window.vnilink.nativeHostCommand(${JSON.stringify(method)}, [info.windowId${args}]))`,
		);
	window.show();
	await call('focusWindow');
	await call('maximizeWindow');
	await call('unmaximizeWindow');
	// Fullscreen transitions are asynchronous on macOS. Check the renderer after each native event.
	for (const fullscreen of [true, false]) {
		const transitioned = new Promise<void>((resolve, reject) => {
			const event = fullscreen ? 'enter-full-screen' : 'leave-full-screen';
			const timer = setTimeout(() => {
				window.removeListener(event as 'enter-full-screen', done);
				reject(new Error('Fullscreen transition timed out'));
			}, 10000);
			function done(): void {
				clearTimeout(timer);
				resolve();
			}
			window.once(event as 'enter-full-screen', done);
		});
		await call('setFullScreen', ', ' + String(fullscreen));
		await transitioned;
		const hidden: unknown = await window.webContents.executeJavaScript(
			`new Promise((resolve, reject) => { const start = performance.now(); function check() { const bar = document.querySelector('.part.titlebar'); if (bar.hidden === ${String(fullscreen)}) { resolve(bar.hidden); } else if (performance.now() - start > 5000) { reject(new Error('Titlebar fullscreen layout did not update')); } else { requestAnimationFrame(check); } } check(); })`,
		);
		assert.equal(hidden, fullscreen);
	}
	const closedView: unknown = await window.webContents.executeJavaScript(`(async () => {
    const panel = document.querySelector('.view-host > [role="tabpanel"]');
    if (!panel?.querySelector('.sidebar')) throw new Error('Sidebar is outside its tab view');
    document.querySelector('.tabs-container .tab-close').click();
    await new Promise(resolve => setTimeout(resolve, 0));
    return !document.querySelector('.sidebar') && !document.querySelector('[role="tabpanel"]') && !document.querySelector('.view-action');
  })()`);
	assert.equal(closedView, true, 'Closing Computers left its interface behind');
	await call('minimizeWindow');
	await call('focusWindow');
	const state: unknown = await call('getWindowState');
	assert.ok(
		record(state) && state.windowId === window.id && state.fullscreen === false && state.alwaysOnTop === false,
	);
}
void startApplication({ showWindow: false, temporaryEngine: true, onReady: verify });
