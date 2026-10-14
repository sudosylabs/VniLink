import { defaultConfiguration } from '../../src/platform/configuration/common/configuration.js';
import { isPermissionSnapshot } from '../../src/platform/permissions/common/permissions.js';
import assert from 'node:assert/strict';
import { getMacOSWindowControlsPosition } from '../../src/platform/window/common/window.js';
import { getWorkbenchTitlebarHeight } from '../../src/platform/window/common/titlebar.js';
import { isEngineReply, record } from '../../src/platform/engine/common/engine.js';
import type { BrowserWindow } from 'electron';
import { app, net, nativeTheme } from 'electron';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { release } from 'node:os';
import { isMacintosh } from '../../src/base/common/platform.js';
import { isAppInfo } from '../../src/platform/ipc/common/desktopApi.js';
import { TestApplication } from '../automation/application.js';
import type { WindowDriver } from '../automation/driver.js';
import { ScenarioSelection, browserScenarioNames } from '../automation/scenarios.js';
import { registerDevelopmentWindow } from '../../src/app/electron-main/development.js';

class SmokeSuite {
	private readonly application = new TestApplication();
	private readonly selection = new ScenarioSelection(this.application.args);

	public run(): Promise<void> {
		return this.application.run((window, driver) => this.verify(window, driver));
	}

	private async verify(window: BrowserWindow, driver: WindowDriver): Promise<void> {
		await driver.ready();
		const result: unknown = await window.webContents.executeJavaScript(`(async () => {
    const info = await window.vnilink.getAppInfo();
    return { info, nodeExposed: typeof require !== 'undefined' || typeof process !== 'undefined',
      title: document.querySelector('h1')?.textContent, noticeHidden: document.querySelector('.workbench-notice')?.hidden, api: Object.keys(window.vnilink).sort() };
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
			result.title !== 'Devices' ||
			!('noticeHidden' in result) || result.noticeHidden !== true ||
			!('api' in result) ||
			JSON.stringify(result.api) !==
			JSON.stringify(['decryptText', 'encryptText', 'engineCommand', 'getAppInfo', 'getCaptureDisplays', 'getConfiguration', 'getKeyStorageProvider', 'getRestartState', 'isEncryptionAvailable', 'nativeHostCommand', 'onConfigurationChanged', 'onRestartStateChanged', 'onWindowStateChanged', 'openDataChannel', 'openRelayDocumentation', 'permissionCommand', 'restartApplication', 'updateConfiguration'])
		) {
			throw new Error(`Unexpected renderer state: ${JSON.stringify(result)}`);
		}
		const machinePermissions: unknown = await window.webContents.executeJavaScript(`window.vnilink.permissionCommand({ type: 'status' })`);
		if (!isPermissionSnapshot(machinePermissions)) {
			throw new Error('Invalid live OS permission snapshot');
		}
		for (const name of browserScenarioNames) {
			if (this.selection.includes(name)) {
				await driver.browserScenario(name);
				await driver.screenshot(name);
			}
		}
		if (!this.selection.names.some((name: string) => ['engine', 'navigation', 'window', 'appearance', 'reload'].includes(name))) {
			return;
		}
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
		if (this.selection.includes('navigation')) {
			await this.verifyHomeNavigation(window);
			await driver.screenshot('navigation');
		}
		if (this.selection.includes('window')) {
			await this.verifyTitlebar(window);
			await driver.screenshot('window');
		}
		if (this.selection.includes('appearance')) {
			await this.verifyPreferences(window);
			await driver.screenshot('appearance');
		}
		if (this.selection.includes('reload')) {
			this.enableDevelopmentReload(window);
			for (let index = 0; index < 3; index++) {
				await this.verifyDevelopmentReload(window, engine.snapshot.snapshot.endpoint_id);
				await driver.ready();
			}
			await driver.screenshot('reload');
		}
		console.log(`PASS: ${this.selection.names.join(', ')}`);
	}

	private async verifyHomeNavigation(window: BrowserWindow): Promise<void> {
		const result: unknown = await window.webContents.executeJavaScript(`(() => {
      const sidebar = document.querySelector('.sidebar');
      const labels = [...sidebar.querySelectorAll('.nav-item')].map(button => button.textContent.trim());
      if (document.querySelector('.status, .brand, footer')) throw new Error('Old branding or status bar remains');
      const devices = document.querySelector('.home-content > main');
      const pages = new Map();
      for (const button of sidebar.querySelectorAll('.nav-item:not([data-section-id="devices"])')) {
        button.click();
        if (document.querySelector('h1').textContent !== button.textContent.trim()) throw new Error('Sidebar did not navigate');
        if (sidebar.querySelectorAll('[aria-current="page"]').length !== 1) throw new Error('Selection is ambiguous');
        if (button.querySelector('.vnilink-icon').dataset.variant !== 'filled') throw new Error('Selected sidebar icon is not filled');
        if ([...sidebar.querySelectorAll('.nav-item:not([aria-current]) .vnilink-icon')].some(icon => icon.dataset.variant !== 'outlined')) throw new Error('Unselected icons are not outlined');
        if (document.querySelector('.view-action')) throw new Error('Device actions remained on Settings');
        pages.set(button.dataset.sectionId, document.querySelector('.home-content > main'));
      }
      for (const button of sidebar.querySelectorAll('.nav-item:not([data-section-id="devices"])')) {
        button.click();
        if (document.querySelector('.home-content > main') !== pages.get(button.dataset.sectionId)) throw new Error('Settings page state was discarded');
      }
      sidebar.querySelector('[data-section-id="devices"]').click();
      if (document.querySelector('.home-content > main') !== devices) throw new Error('Devices view was discarded');
      if (sidebar.querySelector('[data-section-id="devices"] .vnilink-icon').dataset.icon !== 'devices') throw new Error('Wrong Devices artwork');
      if (sidebar.querySelector('[data-section-id="desktop"] .vnilink-icon').dataset.icon !== 'remoteDesktop') throw new Error('Wrong Remote Desktop artwork');
      return { labels, settings: sidebar.querySelector('.sidebar-heading').textContent,
        fullHeight: Math.abs(sidebar.getBoundingClientRect().bottom - innerHeight) < 1,
        selected: sidebar.querySelector('[aria-current="page"]').dataset.sectionId,
        refresh: document.querySelector('.view-action').getAttribute('aria-label') };
    })()`);
		assert.deepEqual(result, {
			labels: ['Devices', 'Appearance', 'Connection', 'Access & Security', 'Remote Desktop', 'Keyboard & Mouse', 'Terminal'],
			settings: 'Settings', fullHeight: true, selected: 'devices', refresh: 'Refresh devices',
		});
	}

	private async verifyPreferences(window: BrowserWindow): Promise<void> {
		await window.webContents.executeJavaScript(`(async () => {
    await window.vnilink.updateConfiguration({ key: 'appearance.theme', value: 'dark' });
    await window.vnilink.updateConfiguration({ key: 'appearance.nativeBackground', value: false });
    await new Promise((resolve, reject) => {
      const start = performance.now();
      function check() {
        if (document.documentElement.dataset.windowMaterial === 'none' && getComputedStyle(document.querySelector('.home-content')).backgroundColor === 'rgb(32, 33, 36)') { resolve(); }
        else if (performance.now() - start > 5000) { reject(new Error('Persisted appearance did not apply')); }
        else { requestAnimationFrame(check); }
      }
      check();
    });
    let rejected = false;
    try { await window.vnilink.updateConfiguration({ key: 'appearance.theme', value: 'invalid' }); } catch { rejected = true; }
    if (!rejected) throw new Error('Invalid preference accepted');
    return true;
  })()`);
		assert.deepEqual(JSON.parse(readFileSync(join(this.application.profile, 'settings.json'), 'utf8')), { ...defaultConfiguration, 'appearance.theme': 'dark', 'appearance.nativeBackground': false });
		assert.equal(nativeTheme.themeSource, 'dark');
		const size = window.getSize();
		try {
			window.setSize(760, 520);
			const layout: unknown = await window.webContents.executeJavaScript(`new Promise(resolve => {
      document.querySelector('[data-section-id="appearance"]').click();
      requestAnimationFrame(() => {
        const content = document.querySelector('.home-content');
        const picker = document.querySelector('.theme-mode-picker');
        const bounds = content.getBoundingClientRect();
        const choices = [...picker.querySelectorAll('.theme-choice')];
        resolve({ choices: choices.length, selected: picker.querySelector('input:checked').value,
          overflow: content.scrollWidth > content.clientWidth,
          contained: choices.every(choice => { const rect = choice.getBoundingClientRect(); return rect.left >= bounds.left && rect.right <= bounds.right; }),
          headingVisible: document.querySelector('.appearance-page h1').getBoundingClientRect().top >= bounds.top,
          switch: document.querySelectorAll('.appearance-page [role="switch"]').length });
      });
    })`);
			assert.deepEqual(layout, { choices: 3, selected: 'dark', overflow: false, contained: true, headingVisible: true, switch: 1 });
		} finally {
			window.setSize(size[0], size[1]);
		}
		await window.webContents.executeJavaScript(`(async () => {
    await window.vnilink.updateConfiguration({ key: 'appearance.theme', value: 'system' });
    await window.vnilink.updateConfiguration({ key: 'appearance.nativeBackground', value: true });
    await new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)));
    if (!document.querySelector('.workbench-notice').hidden) throw new Error('Native theme update raised an application error');
  })()`);
	}

	private enableDevelopmentReload(window: BrowserWindow): void {
		const previous = process.env.VNILINK_DEV;
		process.env.VNILINK_DEV = '1';
		try {
			registerDevelopmentWindow(window);
		} finally {
			if (previous === undefined) {
				delete process.env.VNILINK_DEV;
			} else {
				process.env.VNILINK_DEV = previous;
			}
		}
	}

	private async verifyDevelopmentReload(window: BrowserWindow, endpointId: string): Promise<void> {
		const loaded = new Promise<void>((resolve, reject) => {
			const timer = setTimeout(() => {
				window.webContents.removeListener('did-finish-load', done);
				reject(new Error('Development reload timed out'));
			}, 5000);
			function done(): void {
				clearTimeout(timer);
				resolve();
			}
			window.webContents.once('did-finish-load', done);
		});
		let prevented = false;
		window.webContents.emit('before-input-event', {
			preventDefault: () => {
				prevented = true;
			}
		}, { type: 'keyDown', key: 'r', meta: isMacintosh, control: !isMacintosh, alt: false, shift: false });
		await loaded;
		assert.equal(prevented, true);
		const snapshot: unknown = await window.webContents.executeJavaScript(`window.vnilink.engineCommand({ type: 'snapshot' })`);
		assert.ok(isEngineReply(snapshot) && snapshot.type === 'snapshot' && snapshot.snapshot.endpoint_id === endpointId, 'Reload replaced the native engine');
	}

	private async verifyTitlebar(window: BrowserWindow): Promise<void> {
		const height = getWorkbenchTitlebarHeight(release());
		const presentation: unknown = await window.webContents.executeJavaScript(
			`({ height: document.querySelector('.part.titlebar').getBoundingClientRect().height, drag: getComputedStyle(document.querySelector('.titlebar-drag-region')).getPropertyValue('-webkit-app-region'), unwanted: document.querySelectorAll('.window-controls, .titlebar-center, .menubar').length })`,
		);
		assert.deepEqual(presentation, { height, drag: 'drag', unwanted: 0 });
		const tabs: unknown = await window.webContents.executeJavaScript(`(() => {
      const bar = document.querySelector('.part.titlebar').getBoundingClientRect();
      const tab = document.querySelector('.tab[aria-selected="true"]');
      const bounds = tab.getBoundingClientRect();
      const region = selector => getComputedStyle(document.querySelector(selector)).getPropertyValue('-webkit-app-region');
      return { height: bounds.height, topGap: bounds.top - bar.top, bottomGap: bar.bottom - bounds.bottom,
        radius: getComputedStyle(tab.querySelector('.tab-fill')).borderTopLeftRadius,
        strip: region('.tabs-scrollable'), container: region('.tabs-container'), tab: region('.tab'),
        close: tab.querySelector('.tab-close') !== null, action: region('.view-action') };
    })()`);
		assert.deepEqual(tabs, { height: 35, topGap: height - 35, bottomGap: 0, radius: '12px', strip: 'drag', container: 'drag', tab: 'no-drag', close: false, action: 'no-drag' });
		const surfaces: unknown = await window.webContents.executeJavaScript(`(() => {
      const color = selector => getComputedStyle(document.querySelector(selector)).backgroundColor;
      return { mode: document.documentElement.dataset.windowMaterial, titlebar: color('.part.titlebar'), sidebar: color('.sidebar'), content: color('.home-content') };
    })()`);
		assert.ok(record(surfaces) && (surfaces.mode === 'none' || surfaces.mode === 'vibrancy' || surfaces.mode === 'mica'));
		assert.notEqual(surfaces.content, 'rgba(0, 0, 0, 0)', 'View content became transparent');
		if (surfaces.mode !== 'none') {
			const expected = surfaces.mode === 'vibrancy'
				? nativeTheme.shouldUseDarkColors ? 'rgba(32, 33, 36, 0.22)' : 'rgba(248, 248, 250, 0.58)'
				: 'rgba(0, 0, 0, 0)';
			assert.equal(surfaces.titlebar, expected);
			assert.equal(surfaces.sidebar, expected);
			// Electron's getter returns RGB and omits the configured alpha channel.
			assert.equal(window.getBackgroundColor(), '#000000');
		}
		const toolbarGap: unknown = await window.webContents.executeJavaScript(`(() => {
      const container = document.querySelector('.titlebar-container');
      const toolbar = document.querySelector('.view-actions');
      return container.getBoundingClientRect().right - toolbar.getBoundingClientRect().right - parseFloat(getComputedStyle(container).paddingRight);
    })()`);
		assert.ok(typeof toolbarGap === 'number' && Math.abs(toolbarGap) < 0.1, 'Toolbar is inset beyond the native control reservation');
		if (isMacintosh) {
			assert.deepEqual(window.getWindowButtonPosition(), getMacOSWindowControlsPosition(height, release()));
		}
		for (const zoom of [1.25, 1]) {
			window.webContents.setZoomFactor(zoom);
			const expected = height / zoom;
			const zoomed: unknown = await window.webContents.executeJavaScript(`new Promise((resolve, reject) => { const start = performance.now(); function check() { const height = document.querySelector('.part.titlebar').getBoundingClientRect().height; if (Math.abs(height - ${String(expected)}) < 0.1) { resolve(true); } else if (performance.now() - start > 5000) { reject(new Error('Titlebar zoom layout did not update')); } else { requestAnimationFrame(check); } } check(); })`);
			assert.equal(zoomed, true);
			const tabHeight: unknown = await window.webContents.executeJavaScript(`document.querySelector('.tab').getBoundingClientRect().height`);
			assert.ok(typeof tabHeight === 'number' && Math.abs(tabHeight - 35 / zoom) < 0.1, 'Titlebar zoom stretched the tabs');
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
					`new Promise((resolve, reject) => { const start = performance.now(); function check() { if (getComputedStyle(document.querySelector('.part.titlebar')).backgroundColor === (document.documentElement.dataset.windowMaterial === 'none' ? ${JSON.stringify(background)} : document.documentElement.dataset.windowMaterial === 'vibrancy' ? ("${theme}" === 'dark' ? 'rgba(32, 33, 36, 0.22)' : 'rgba(248, 248, 250, 0.58)') : 'rgba(0, 0, 0, 0)') && getComputedStyle(document.querySelector('.home-content')).backgroundColor === ("${theme}" === 'dark' ? 'rgb(32, 33, 36)' : 'rgb(250, 250, 250)')) { resolve(true); } else if (performance.now() - start > 5000) { reject(new Error('Titlebar theme did not update')); } else { requestAnimationFrame(check); } } check(); })`,
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
		const homeView: unknown = await window.webContents.executeJavaScript(`(async () => {
    const tab = document.querySelector('.tabs-container [role="tab"]');
    const panel = document.querySelector('.view-host > [role="tabpanel"]');
    if (!panel?.querySelector('.sidebar')) throw new Error('Sidebar is outside its tab view');
    if (tab.querySelector('.tab-close')) throw new Error('Home has a close button');
    tab.dispatchEvent(new KeyboardEvent('keydown', { key: 'Delete', bubbles: true }));
    tab.dispatchEvent(new MouseEvent('auxclick', { button: 1, bubbles: true }));
    await new Promise(resolve => setTimeout(resolve, 0));
    return { label: tab.querySelector('.tab-label').textContent, permanent: document.body.contains(panel), draggable: tab.draggable };
  })()`);
		assert.deepEqual(homeView, { label: 'Home', permanent: true, draggable: false });
		await call('minimizeWindow');
		await call('focusWindow');
		const state: unknown = await call('getWindowState');
		assert.ok(
			record(state) && state.windowId === window.id && state.fullscreen === false && state.alwaysOnTop === false,
		);
	}


}

void new SmokeSuite().run();
