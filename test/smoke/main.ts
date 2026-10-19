import { isPermissionSnapshot } from '../../src/platform/permissions/common/permissions.js';
import assert from 'node:assert/strict';
import { getMacOSWindowControlsPosition } from '../../src/platform/window/common/window.js';
import { getWorkbenchTitlebarHeight } from '../../src/platform/window/common/titlebar.js';
import { resolveWindowMaterial } from '../../src/platform/window/common/windowMaterial.js';
import { isEngineReply, record } from '../../src/platform/engine/common/engine.js';
import { app, BrowserWindow, net, nativeTheme } from 'electron';
import { appendFileSync, readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { release } from 'node:os';
import { isMacintosh } from '../../src/base/common/platform.js';
import { isNativeWindowConfiguration } from '../../src/platform/window/common/windowConfiguration.js';
import { TestApplication } from '../automation/application.js';
import type { WindowDriver } from '../automation/driver.js';
import { ScenarioSelection, browserScenarioNames } from '../automation/scenarios.js';
import { registerDevelopmentWindow } from '../../src/code/electron-main/development.js';
import { NOTICE_URL } from '../../src/platform/systemActions/common/notice.js';
import { windowPrompt } from '../../src/platform/systemActions/electron-main/noticeWindow.js';

class SmokeSuite {
	private readonly application = new TestApplication();
	private readonly selection = new ScenarioSelection(this.application.args);

	public run(): Promise<void> {
		return this.application.run((window, driver) => this.verify(window, driver));
	}

	/** The restart confirmation as Windows and Linux draw it, in each theme. */
	private async verifyDialogThemes(window: BrowserWindow, driver: WindowDriver): Promise<void> {
		const theme = nativeTheme.themeSource;
		try {
			for (const mode of ['light', 'dark'] as const) {
				nativeTheme.themeSource = mode;
				await window.webContents.executeJavaScript(`browserTests.browserScenarios.showDialogFixture()`);
				await window.webContents.executeJavaScript(`new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)))`);
				await driver.screenshot(`dialogs-${mode}`);
				await window.webContents.executeJavaScript(`browserTests.browserScenarios.closeDialogFixture()`);
				await window.webContents.executeJavaScript(`new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)))`);
				assert.equal(await window.webContents.executeJavaScript(`document.querySelector('.monaco-dialog-box') === null`), true, 'Escape left the dialog open');
			}
		} finally {
			nativeTheme.themeSource = theme;
		}
	}

	/** The notice window counts down from its deadline, answers Cancel, and closes when its signal aborts. */
	private async verifyNotice(): Promise<void> {
		const theme = nativeTheme.themeSource;
		const known = new Set(BrowserWindow.getAllWindows());
		const opened = async (): Promise<BrowserWindow> => {
			for (let attempt = 0; attempt < 100; attempt++) {
				const window = BrowserWindow.getAllWindows().find(candidate => !known.has(candidate) && candidate.isVisible());
				if (window) {
					return window;
				}
				await new Promise(resolve => setTimeout(resolve, 50));
			}
			throw new Error('The notice window did not open');
		};
		const detail = (window: BrowserWindow): Promise<unknown> => window.webContents.executeJavaScript(`document.querySelector('#notice-detail')?.textContent`);
		try {
			for (const mode of ['light', 'dark'] as const) {
				nativeTheme.themeSource = mode;
				const answer = windowPrompt({ action: 'restart', name: 'Studio Mac', deadline: Date.now() + 30_000 }, new AbortController().signal);
				const window = await opened();
				assert.equal(window.webContents.getURL(), NOTICE_URL);
				assert.equal(await window.webContents.executeJavaScript(`document.querySelector('#notice-title')?.textContent`), 'Studio Mac wants to restart this computer');
				assert.equal(await window.webContents.executeJavaScript(`document.activeElement?.textContent`), 'Cancel', 'Cancel is not focused');
				// The workbench's backdrop, or none where the system cannot draw one.
				const backdrop: unknown = await window.webContents.executeJavaScript(`(() => {
					const root = document.documentElement;
					return { material: root.dataset.windowMaterial, root: getComputedStyle(root).backgroundColor, body: getComputedStyle(document.body).backgroundColor };
				})()`);
				assert(typeof backdrop === 'object' && backdrop !== null);
				const { material, root, body } = backdrop as Record<string, string>;
				assert.match(material, /^(none|vibrancy|glass|mica)$/);
				// Over glass or Mica nothing of the page's own paints behind the notice.
				if (material === 'glass' || material === 'mica') {
					assert.deepEqual([root, body], ['rgba(0, 0, 0, 0)', 'rgba(0, 0, 0, 0)'], `The page hides the ${material} backdrop`);
				}
				appendFileSync(join(this.application.output, 'notice.log'), `${mode}: ${material} ${root} ${body}\n`);
				const first = await detail(window);
				assert.match(String(first), /^Restarting in (30|29) s\. Save your work, or cancel\.$/);
				await new Promise(resolve => setTimeout(resolve, 1300));
				assert.notEqual(await detail(window), first, 'The notice did not count down');
				writeFileSync(join(this.application.output, `notice-${mode}.png`), (await window.webContents.capturePage()).toPNG());
				await window.webContents.executeJavaScript(`[...document.querySelectorAll('.monaco-button')].find(button => button.textContent === 'Cancel').click()`);
				assert.equal(await answer, 0, 'Cancel did not answer');
				assert(window.isDestroyed(), 'The answered notice stayed open');
			}
			// Ended another way: the window closes and answers nothing.
			const controller = new AbortController();
			const answer = windowPrompt({ action: 'shutdown', name: 'Studio Mac', deadline: Date.now() + 30_000 }, controller.signal);
			const window = await opened();
			controller.abort();
			assert.equal(await answer, undefined);
			assert(window.isDestroyed(), 'An aborted notice stayed open');
		} finally {
			nativeTheme.themeSource = theme;
		}
	}

	private async verifyDeviceLayouts(window: BrowserWindow, driver: WindowDriver): Promise<void> {
		const size = window.getSize();
		const theme = nativeTheme.themeSource;
		try {
			for (const mode of ['light', 'dark'] as const) {
				nativeTheme.themeSource = mode;
				window.setSize(1400, 900);
				await window.webContents.executeJavaScript(`browserTests.browserScenarios.showDeviceFixture('selected')`);
				await window.webContents.executeJavaScript(`new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)))`);
				await driver.screenshot(`devices-selected-${mode}`);
			}
			// The inspector is a full-height sheet: it scrolls on its own, and the page does not.
			// A short window makes its content taller than the sheet.
			window.setSize(1400, 600);
			await window.webContents.executeJavaScript(`new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)))`);
			const sheet: unknown = await window.webContents.executeJavaScript(`(() => {
				const host = document.querySelector('.device-fixture');
				const panel = host.querySelector('.device-panel');
				const list = host.querySelector('.device-list');
				const before = list.getBoundingClientRect().top;
				panel.scrollTop = panel.scrollHeight;
				const scrolled = panel.scrollTop > 0;
				const listStayed = list.getBoundingClientRect().top === before;
				panel.scrollTop = 0;
				const hostBox = host.getBoundingClientRect();
				const box = panel.getBoundingClientRect();
				return { scrolled, listStayed, pageScrolls: host.scrollHeight > host.clientHeight,
					fullHeight: Math.abs(box.top - hostBox.top) < 1 && Math.abs(box.bottom - hostBox.bottom) < 1,
					atRightEdge: Math.abs(box.right - hostBox.right) < 1 };
			})()`);
			assert.deepEqual(sheet, { scrolled: true, listStayed: true, pageScrolls: false, fullHeight: true, atRightEdge: true }, 'The inspector is not a full-height sheet with its own scroll');
			window.setSize(800, 800);
			await window.webContents.executeJavaScript(`new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)))`);
			const overflow: unknown = await window.webContents.executeJavaScript(`(() => {
				const page = document.querySelector('.device-fixture main');
				return page.scrollWidth > page.clientWidth;
			})()`);
			assert.equal(overflow, false, 'Device panel overflows a narrow window');
			// Narrow: the list keeps the full width and the sheet covers its right side.
			const covering: unknown = await window.webContents.executeJavaScript(`(() => {
				const host = document.querySelector('.device-fixture');
				const list = host.querySelector('.device-list').getBoundingClientRect();
				const panel = host.querySelector('.device-panel').getBoundingClientRect();
				return { overList: panel.left < list.right, atRightEdge: Math.abs(panel.right - host.getBoundingClientRect().right) < 1 };
			})()`);
			assert.deepEqual(covering, { overList: true, atRightEdge: true }, 'A narrow window split the page instead of showing the inspector as a sheet');
			await driver.screenshot('devices-selected-narrow');
			for (const state of ['empty', 'unavailable']) {
				await window.webContents.executeJavaScript(`browserTests.browserScenarios.showDeviceFixture(${JSON.stringify(state)})`);
				await window.webContents.executeJavaScript(`new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)))`);
				await driver.screenshot(`devices-${state}`);
			}
			// Verify Chromium's real file-download path, but cancel before writing.
			const download = new Promise<string>((resolve, reject) => {
				const timer = setTimeout(() => {
					window.webContents.session.removeListener('will-download', listener);
					reject(new Error('Invitation download did not start'));
				}, 5000);
				const listener = (_event: Electron.Event, item: Electron.DownloadItem): void => {
					clearTimeout(timer);
					item.cancel();
					resolve(item.getFilename());
				};
				window.webContents.session.once('will-download', listener);
			});
			const code: unknown = await window.webContents.executeJavaScript(`(async () => {
				browserTests.browserScenarios.showDeviceFixture('empty');
				browserTests.browserScenarios.shareFixtureInvitation();
				const deadline = Date.now() + 3000;
				const code = () => document.querySelector('.device-pairing-popover .device-pairing-code:not([hidden])')?.textContent;
				while (!code()) {
					if (Date.now() > deadline) { throw new Error('The invitation popover did not show its code'); }
					await new Promise(resolve => requestAnimationFrame(resolve));
				}
				document.querySelector('.device-pairing-popover .monaco-button.primary').click();
				return code();
			})()`);
			assert.equal(code, '042');
			assert.equal(await download, 'VniLink.vnl');
			await driver.screenshot('devices-pairing-popover');
			await window.webContents.executeJavaScript(`(async () => {
				browserTests.browserScenarios.requestFixturePairing();
				const popover = document.querySelector('.device-pairing-popover');
				const deadline = Date.now() + 3000;
				while (!popover.querySelector('.device-pairing-field:not([hidden]) input')) {
					if (Date.now() > deadline) { throw new Error('Incoming pairing field did not appear'); }
					await new Promise(resolve => requestAnimationFrame(resolve));
				}
				if (popover.querySelector('.device-pairing-code:not([hidden])')) { throw new Error('Incoming confirmation disclosed the expected code'); }
				const input = popover.querySelector('.device-pairing-field input');
				input.value = '731';
				input.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', keyCode: 13, bubbles: true }));
			})()`);
		} finally {
			await window.webContents.executeJavaScript(`browserTests.browserScenarios.clearDeviceFixture()`);
			nativeTheme.themeSource = theme;
			window.setSize(size[0], size[1]);
		}
	}

	private async verify(window: BrowserWindow, driver: WindowDriver): Promise<void> {
		await driver.ready();
		const result: unknown = await window.webContents.executeJavaScript(`(async () => {
    const info = await window.vscode.context.resolveConfiguration();
    let unprefixedRejected = false;
    try { await window.vscode.ipcRenderer.invoke('engine:command', { type: 'snapshot' }); } catch { unprefixedRejected = true; }
    return { info, nodeExposed: typeof require !== 'undefined' || typeof process !== 'undefined', vnilinkExposed: 'vnilink' in window, unprefixedRejected,
      title: document.querySelector('h1')?.textContent, noticeHidden: document.querySelector('.workbench-notice')?.hidden, api: Object.keys(window.vscode).sort() };
  })()`);
		if (
			typeof result !== 'object' ||
			result === null ||
			!('nodeExposed' in result) ||
			result.nodeExposed !== false ||
			!('info' in result) ||
			!isNativeWindowConfiguration(result.info) ||
			!('vnilinkExposed' in result) || result.vnilinkExposed !== false ||
			!('unprefixedRejected' in result) || result.unprefixedRejected !== true ||
			result.info.version !== app.getVersion() ||
			!('title' in result) ||
			result.title !== 'Devices' ||
			!('noticeHidden' in result) || result.noticeHidden !== true ||
			!('api' in result) ||
			JSON.stringify(result.api) !==
			JSON.stringify(['context', 'ipcMessagePort', 'ipcRenderer', 'process'])
		) {
			throw new Error(`Unexpected renderer state: ${JSON.stringify(result)}`);
		}
		const machinePermissions: unknown = await window.webContents.executeJavaScript(`window.driver.permissions.execute({ type: 'status' })`);
		if (!isPermissionSnapshot(machinePermissions)) {
			throw new Error('Invalid live OS permission snapshot');
		}
		for (const name of browserScenarioNames) {
			if (this.selection.includes(name)) {
				await driver.browserScenario(name);
				await driver.screenshot(name);
				if (name === 'devices') {
					await this.verifyDeviceLayouts(window, driver);
				}
				if (name === 'dialogs') {
					await this.verifyDialogThemes(window, driver);
				}
			}
		}
		if (this.selection.includes('notice')) {
			await this.verifyNotice();
		}
		if (!this.selection.names.some((name: string) => ['engine', 'navigation', 'window', 'appearance', 'settings-layout', 'reload'].includes(name))) {
			return;
		}
		const engine: unknown = await window.webContents.executeJavaScript(`(async () => {
    const engine = window.driver.engine;
    const snapshot = await engine.execute({ type: 'snapshot' });
    const invitation = await engine.execute({ type: 'createInvitation', ttlSeconds: 60 });
    let invalidRejected = false;
    try { await engine.execute({ type: 'shutdown' }); } catch { invalidRejected = true; }
    let sessionError = '';
    try { await engine.execute({ type: 'disconnect' }); } catch (error) { sessionError = String(error); }
    const afterError = await engine.execute({ type: 'snapshot' });
    const hostingEvent = new Promise((resolve, reject) => {
      const timer = setTimeout(() => { listener.dispose(); reject(new Error('No hosting event arrived')); }, 5000);
      const listener = engine.onDidReceiveEvents(batch => {
        const event = batch.events.find(candidate => candidate.type === 'hostingChanged' && candidate.hosting === false);
        if (event) { clearTimeout(timer); listener.dispose(); resolve(event); }
      });
    });
    // Pausing sharing is a saved setting; main turns hosting off and the event reaches this window.
    const configuration = window.driver.mainProcess.getChannel('configuration');
    await configuration.call('updateValue', { key: 'application.acceptConnections', value: false });
    const firstBatch = await hostingEvent;
    await configuration.call('updateValue', { key: 'application.acceptConnections', value: true });
    let privateRejected = false;
    try { await engine.execute({ type: 'subscribeEvents' }); } catch { privateRejected = true; }
    let hostingRejected = false;
    try { await engine.execute({ type: 'stopHosting' }); } catch { hostingRejected = true; }
    let unknownChannelRejected = false;
    try { await window.driver.mainProcess.getChannel('encryption').call('encrypt', 'secret'); } catch { unknownChannelRejected = true; }
    return { snapshot, invitation, invalidRejected, sessionError, afterError, unknownChannelRejected, firstBatch, privateRejected, hostingRejected };
  })()`);
		if (
			!record(engine) ||
			!isEngineReply(engine.snapshot) ||
			engine.snapshot.type !== 'snapshot' ||
			engine.snapshot.snapshot.endpoint_id.length !== 64 ||
			!engine.snapshot.snapshot.hosting ||
			!isEngineReply(engine.invitation) ||
			engine.invitation.type !== 'invitation' ||
			!engine.invitation.token ||
			engine.invalidRejected !== true ||
			typeof engine.sessionError !== 'string' ||
			!engine.sessionError.includes('NoSession') ||
			!isEngineReply(engine.afterError) ||
			engine.afterError.type !== 'snapshot' ||
			engine.unknownChannelRejected !== true ||
			!record(engine.firstBatch) || engine.firstBatch.type !== 'hostingChanged' ||
			engine.privateRejected !== true ||
			engine.hostingRejected !== true
		) {
			throw new Error('Native control bridge failed');
		}
		let rejected = false;
		try {
			await window.webContents.executeJavaScript(`window.driver.mainProcess.getChannel('nativeHost').call('invalid', [window.driver.nativeHost.windowId])`);
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
		if (this.selection.includes('settings-layout')) {
			await this.verifySettingsLayout(window, driver);
		}
		if (this.selection.includes('reload')) {
			this.enableDevelopmentReload(window);
			for (let index = 0; index < 3; index++) {
				await this.verifyDevelopmentReload(window, driver, engine.snapshot.snapshot.endpoint_id);
			}
			await driver.screenshot('reload');
		}
		console.log(`PASS: ${this.selection.names.join(', ')}`);
	}

	private async verifySettingsLayout(window: BrowserWindow, driver: WindowDriver): Promise<void> {
		const size = window.getSize();
		try {
			for (const theme of ['light', 'dark']) {
				await window.webContents.executeJavaScript(`window.driver.configuration.updateValue('appearance.theme', ${JSON.stringify(theme)})`);
				for (const width of [1200, 760]) {
					window.setSize(width, 820);
					for (const category of ['general', 'appearance', 'access', 'desktop', 'input', 'terminal', 'media', 'recording', 'power', 'connection']) {
						const layout: unknown = await window.webContents.executeJavaScript(`new Promise(resolve => {
       document.querySelector('[data-section-id="${category}"]').click();
       requestAnimationFrame(() => requestAnimationFrame(() => {
        const content = document.querySelector('.home-content');
        const page = content.querySelector('main');
        const buttons = [...page.querySelectorAll('button')];
        resolve({ note: page.querySelector('.settings-page-note')?.textContent.trim() ?? '', submit: !!page.querySelector('button[type="submit"]'), overflow: content.scrollWidth > content.clientWidth, heading: page.querySelector('h1').textContent, refresh: buttons.some(button => button.textContent === 'Refresh'), session: page.textContent.includes('Session overrides'), device: page.textContent.includes('Device access'), legacyRows: page.querySelectorAll('.settings-control-row, .access-toggle-row, .access-section').length });
       }));
      })`);
						assert(record(layout) && layout.overflow === false, `${category} overflowed at ${String(width)}px`);
						assert.equal(layout.submit, false, `${category} still saves through a button`);
						assert.equal(layout.refresh, false, `${category} offers a Refresh button`);
						// Each fact is said once, in its row or section; pages carry no closing note.
						assert.equal(layout.note, '', `${category} has a page note`);
						assert.equal(layout.legacyRows, 0, `${category} uses a retired row layout`);
						if (category === 'access') {
							assert.equal(layout.device, false);
						}
						if (category === 'desktop') {
							assert.equal(layout.session, false);
						}
						await window.webContents.executeJavaScript(`new Promise((resolve, reject) => {
       const deadline = performance.now() + 10000;
       const check = () => {
        const page = document.querySelector('.home-content > main');
        if (page.getAttribute('aria-busy') !== 'true') { resolve(true); }
        else if (performance.now() > deadline) { reject(new Error('Settings read did not settle')); }
        else { setTimeout(check, 20); }
       };
       check();
      })`);
						await driver.screenshot(`settings-${category}-${theme}-${String(width)}`);
					}
				}
			}
		} finally {
			window.setSize(size[0], size[1]);
			await window.webContents.executeJavaScript(`window.driver.configuration.updateValue('appearance.theme', 'system')`);
		}
	}

	private async verifyHomeNavigation(window: BrowserWindow): Promise<void> {
		const result: unknown = await window.webContents.executeJavaScript(`(async () => {
      // Menus follow context keys after a short debounce.
      const settled = async (condition, message) => {
        const deadline = Date.now() + 2000;
        while (!condition()) {
          if (Date.now() > deadline) throw new Error(message);
          await new Promise(resolve => setTimeout(resolve, 16));
        }
      };
      const deviceActions = () => document.querySelectorAll('.view-action[data-action-id^="vnilink.devices."]').length;
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
        await settled(() => deviceActions() === 0, 'Device actions remained on Settings');
        pages.set(button.dataset.sectionId, document.querySelector('.home-content > main'));
      }
      for (const button of sidebar.querySelectorAll('.nav-item:not([data-section-id="devices"])')) {
        button.click();
        if (document.querySelector('.home-content > main') !== pages.get(button.dataset.sectionId)) throw new Error('Settings page state was discarded');
      }
      sidebar.querySelector('[data-section-id="devices"]').click();
      if (document.querySelector('.home-content > main') !== devices) throw new Error('Devices view was discarded');
      await settled(() => deviceActions() === 2, 'Device actions did not return on Devices');
      if (sidebar.querySelector('[data-section-id="devices"] .vnilink-icon').dataset.icon !== 'devices') throw new Error('Wrong Devices artwork');
      if (sidebar.querySelector('[data-section-id="desktop"] .vnilink-icon').dataset.icon !== 'remoteDesktop') throw new Error('Wrong Remote Desktop artwork');
      return { labels, settings: sidebar.querySelector('.sidebar-heading').textContent,
        fullHeight: Math.abs(sidebar.getBoundingClientRect().bottom - innerHeight) < 1,
        selected: sidebar.querySelector('[aria-current="page"]').dataset.sectionId,
        actions: [...document.querySelectorAll('.view-action')].map(action => action.getAttribute('aria-label')) };
    })()`);
		assert.deepEqual(result, {
			labels: ['Devices', 'General', 'Appearance', 'Connection', 'Access & Security', 'Remote Desktop', 'Keyboard & Mouse', 'Terminal', 'Audio & Camera', 'Recording', 'Power'],
			settings: 'Settings', fullHeight: true, selected: 'devices', actions: ['Open an invitation', 'Invite another computer'],
		});
	}

	private async verifyPreferences(window: BrowserWindow): Promise<void> {
		await window.webContents.executeJavaScript(`(async () => {
    const material = document.documentElement.dataset.windowMaterial;
    await window.driver.configuration.updateValue('appearance.theme', 'dark');
    await new Promise((resolve, reject) => {
      const start = performance.now();
      function check() {
        if (document.documentElement.dataset.windowMaterial === material && getComputedStyle(document.querySelector('.home-content')).backgroundColor === 'rgb(32, 33, 36)') { resolve(); }
        else if (performance.now() - start > 5000) { reject(new Error('Persisted appearance did not apply')); }
        else { requestAnimationFrame(check); }
      }
      check();
    });
    let rejected = false;
    try { await window.driver.configuration.updateValue('appearance.theme', 'invalid'); } catch { rejected = true; }
    if (!rejected) throw new Error('Invalid preference accepted');
    return true;
  })()`);
		assert.deepEqual(JSON.parse(readFileSync(join(this.application.profile, 'settings.json'), 'utf8')), { 'appearance.theme': 'dark' });
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
			assert.deepEqual(layout, { choices: 3, selected: 'dark', overflow: false, contained: true, headingVisible: true, switch: 0 });
		} finally {
			window.setSize(size[0], size[1]);
		}
		await window.webContents.executeJavaScript(`(async () => {
    await window.driver.configuration.updateValue('appearance.theme', 'system');
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

	private async verifyDevelopmentReload(window: BrowserWindow, driver: WindowDriver, endpointId: string): Promise<void> {
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
		await driver.ready();
		const snapshot: unknown = await window.webContents.executeJavaScript(`window.driver.engine.execute({ type: 'snapshot' })`);
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
        fill: getComputedStyle(tab.querySelector('.tab-fill')).backgroundColor,
        strip: region('.tabs-scrollable'), container: region('.tabs-container'), tab: region('.tab'),
        close: tab.querySelector('.tab-close') !== null, action: document.querySelector('.view-action') ? region('.view-action') : null };
    })()`);
		assert.deepEqual(tabs, { height: 35, topGap: height - 35, bottomGap: 0, radius: '12px', fill: 'rgba(0, 0, 0, 0)', strip: 'drag', container: 'drag', tab: 'no-drag', close: false, action: 'no-drag' });
		const surfaces: unknown = await window.webContents.executeJavaScript(`(() => {
      const color = selector => getComputedStyle(document.querySelector(selector)).backgroundColor;
      return { mode: document.documentElement.dataset.windowMaterial, titlebar: color('.part.titlebar'), sidebar: color('.sidebar'), content: color('.home-content') };
    })()`);
		// The material this OS should get: Liquid Glass on macOS 26 and later, so a silent
		// fallback to vibrancy (elg not staged or refusing) fails here.
		const material = resolveWindowMaterial(process.platform, release(), nativeTheme.prefersReducedTransparency, nativeTheme.shouldUseHighContrastColors);
		assert.ok(record(surfaces));
		assert.equal(surfaces.mode, material, 'The window did not get its native material');
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
			`(async () => { const windowId = window.driver.nativeHost.windowId; const channel = window.driver.mainProcess.getChannel('nativeHost'); let rejected = 0; for (const [method, args] of [['getWindowState', [windowId + 1000]], ['updateWindowControls', [windowId, { height: -1 }]], ['focusWindow', [windowId, { targetWindowId: windowId + 1000 }]], ['updateWindowControls', [windowId, { arbitrary: true }]], ['openExternal', [windowId, 'https://example.com/']]]) { try { await channel.call(method, args); } catch { rejected++; } } return rejected; })()`,
		);
		assert.equal(rejected, 5);
		// Exercise the event transport and unsubscribe using a real Electron window state change.
		await window.webContents.executeJavaScript(
			`globalThis.__smokeWindowState = new Promise((resolve, reject) => { const timeout = setTimeout(() => { stop(); reject(new Error('Window state event not received')); }, 5000); const listener = window.driver.nativeHost.onDidChangeWindowState(state => { if (state.alwaysOnTop) { clearTimeout(timeout); stop(); resolve(state); } }); function stop() { listener.dispose(); } }); true`,
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
				`window.driver.mainProcess.getChannel('nativeHost').call(${JSON.stringify(method)}, [window.driver.nativeHost.windowId${args}])`,
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
