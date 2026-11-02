// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'assert';
import { Emitter } from '../../../../base/common/event.js';
import { DisposableStore, toDisposable, type IDisposable } from '../../../../base/common/lifecycle.js';
import { ensureNoDisposablesAreLeakedInTestSuite } from '../../../../base/test/common/utils.js';
import { IHostingMainService, type HostingMainService, type HostingStatus } from '../../../hosting/electron-main/hostingMainService.js';
import { TestInstantiationService } from '../../../instantiation/test/common/instantiationServiceMock.js';
import { ILogService, NullLogService } from '../../../log/common/log.js';
import { product } from '../../../product/common/product.js';
import { trayImageName, trayMenu, TrayMainService, trayState, trayStatusLine, type TrayAction, type TrayEnvironment, type TrayIcon, type TrayMenuEntry } from '../../electron-main/trayMainService.js';
import { traySupported } from '../../electron-main/traySupport.js';

const ready: HostingStatus = { available: true, accepting: true, hosting: true, sessions: [] };

const connected: HostingStatus = { ...ready, sessions: [{ id: '7', name: 'Work laptop', phase: 'Active' }, { id: '9', name: 'host-b', phase: 'Negotiating' }] };

const paused: HostingStatus = { ...ready, accepting: false, hosting: false };

function labels(menu: readonly TrayMenuEntry[]): string[] {
	return menu.flatMap(entry => 'label' in entry ? [entry.label] : []);
}

suite('tray presentation', () => {
	const states: readonly (readonly [description: string, status: HostingStatus, state: ReturnType<typeof trayState>])[] = [
		['a computer ready for paired devices is idle', ready, 'idle'],
		['a computer someone is connected to is active', connected, 'active'],
		['a computer that stopped accepting connections is paused', paused, 'paused'],
		['a computer not hosting yet is paused, not ready', { ...ready, hosting: false }, 'paused'],
		['a computer whose engine is unavailable is paused', { ...ready, available: false, hosting: false }, 'paused'],
	];
	for (const [description, status, state] of states) {
		test(description, () => {
			assert.strictEqual(trayState(status), state);
		});
	}
	const lines: readonly (readonly [status: HostingStatus, line: string])[] = [
		[ready, 'Ready for paired devices'],
		[connected, '2 devices connected'],
		[{ ...connected, sessions: connected.sessions.slice(0, 1) }, '1 device connected'],
		[{ ...ready, accepting: false }, 'Not accepting connections'],
		[{ ...ready, available: false }, 'Connection engine unavailable'],
	];
	for (const [status, line] of lines) {
		test(`the status line says "${line}"`, () => {
			assert.strictEqual(trayStatusLine(status), line);
		});
	}
	test('the menu offers a disconnect per device, stopping connections, and Open and Quit', () => {
		assert.deepStrictEqual(labels(trayMenu(connected)), ['2 devices connected', 'Connected to this computer', 'Disconnect Work laptop', 'Disconnect host-b (connecting)', 'Stop Accepting Connections', 'Open VniLink', 'Quit VniLink']);
	});
	test('each disconnect entry ends its own session', () => {
		const sessions = trayMenu(connected).flatMap(entry => 'action' in entry && entry.action?.kind === 'disconnect' ? [entry.action.sessionId] : []);
		assert.deepStrictEqual(sessions, ['7', '9']);
	});
	test('the status line is not an action', () => {
		const [status] = trayMenu(connected);
		assert.ok('enabled' in status && !status.enabled);
	});
	test('a paused computer is offered accepting connections again', () => {
		assert.deepStrictEqual(labels(trayMenu(paused)), ['Not accepting connections', 'Accept Connections', 'Open VniLink', 'Quit VniLink']);
	});
	test('nothing can be changed while the engine is unavailable', () => {
		assert.deepStrictEqual(labels(trayMenu({ available: false, accepting: true, hosting: false, sessions: [] })), ['Connection engine unavailable', 'Open VniLink', 'Quit VniLink']);
	});
	const images: readonly (readonly [description: string, args: Parameters<typeof trayImageName>, image: string])[] = [
		['macOS uses a template image the menu bar tints', ['active', 'darwin', true], 'activeTemplate.png'],
		['Windows with a dark taskbar uses the dark artwork', ['idle', 'win32', true], 'idle-dark.png'],
		['Windows with a light taskbar uses the light artwork', ['idle', 'win32', false], 'idle-light.png'],
		['Linux panels are taken to be dark', ['paused', 'linux', false], 'paused-dark.png'],
	];
	for (const [description, args, image] of images) {
		test(description, () => {
			assert.strictEqual(trayImageName(...args), image);
		});
	}
	test('the service asks only for artwork the build renders', () => {
		// The names `build/lib/trayAssets.ts` renders at 1x; its own test checks the files and sizes.
		const rendered = ['idleTemplate.png', 'activeTemplate.png', 'pausedTemplate.png', 'idle-dark.png', 'active-dark.png', 'paused-dark.png', 'idle-light.png', 'active-light.png', 'paused-light.png'];
		const requested = new Set((['idle', 'active', 'paused'] as const).flatMap(state => (['darwin', 'win32', 'linux'] as const).flatMap(platform => [true, false].map(dark => trayImageName(state, platform, dark)))));
		assert.deepStrictEqual([...requested].sort(), rendered.sort());
	});
	ensureNoDisposablesAreLeakedInTestSuite();
});

suite('traySupported', () => {
	test('macOS and Windows always show tray icons', async () => {
		assert.strictEqual(await traySupported(() => Promise.reject(new Error('gdbus is not asked')), false), true);
	});
	test('Linux shows one while a StatusNotifierItem watcher owns its bus name', async () => {
		assert.strictEqual(await traySupported(() => Promise.resolve({ stdout: '(true,)\n' }), true), true);
	});
	test('Linux shows none while no watcher owns the name', async () => {
		assert.strictEqual(await traySupported(() => Promise.resolve({ stdout: '(false,)\n' }), true), false);
	});
	test('Linux without gdbus cannot know, so shows none', async () => {
		assert.strictEqual(await traySupported(() => Promise.reject(Object.assign(new Error('spawn gdbus ENOENT'), { code: 'ENOENT' })), true), false);
	});
	ensureNoDisposablesAreLeakedInTestSuite();
});

/** A tray icon recording what it shows; `choose` runs a menu entry and `click` clicks the icon. */
class TestTrayIcon implements TrayIcon {
	public readonly images: string[] = [];
	public tooltip = '';
	public menus = 0;
	public destroyed = false;
	private entries: readonly TrayMenuEntry[] = [];
	private run: ((action: TrayAction) => void) | undefined;
	private readonly clicked = new Emitter<void>();

	constructor(image: string) {
		this.images.push(image);
	}

	public isDestroyed(): boolean {
		return this.destroyed;
	}

	public setImage(image: string): void {
		this.images.push(image);
	}

	public setToolTip(text: string): void {
		this.tooltip = text;
	}

	public setMenu(entries: readonly TrayMenuEntry[], run: (action: TrayAction) => void): void {
		this.menus++;
		this.entries = entries;
		this.run = run;
	}

	public onDidClick(listener: () => void): IDisposable {
		return this.clicked.event(listener);
	}

	public get listensForClicks(): boolean {
		return this.clicked.hasListeners();
	}

	public choose(label: string): void {
		const entry = this.entries.find(candidate => 'label' in candidate && candidate.label === label);
		if (!entry || !('action' in entry) || !entry.action || !this.run) {
			throw new Error(`No menu entry ${label}`);
		}
		this.run(entry.action);
	}

	public click(): void {
		this.clicked.fire();
	}

	public destroy(): void {
		this.destroyed = true;
		this.clicked.dispose();
	}
}

/** A system with one tray; `updateTheme` changes the panel's theme and announces it. */
class TestTrayEnvironment implements TrayEnvironment {
	public readonly icons: TestTrayIcon[] = [];
	public quits = 0;
	private readonly themeChanged = new Emitter<void>();

	constructor(public readonly platform: TrayEnvironment['platform'], public darkPanel = true) { }

	public get listensForTheme(): boolean {
		return this.themeChanged.hasListeners();
	}

	public onDidChangeTheme(listener: () => void): IDisposable {
		return this.themeChanged.event(listener);
	}

	public createIcon(image: string): TestTrayIcon {
		const icon = new TestTrayIcon(image);
		this.icons.push(icon);
		return icon;
	}

	public quit(): void {
		this.quits++;
	}

	public updateTheme(darkPanel: boolean): void {
		this.darkPanel = darkPanel;
		this.themeChanged.fire();
	}

	public dispose(): void {
		this.themeChanged.dispose();
	}
}

/** Hosting whose status the test sets; the tray's requests are kept in `requests`. */
class TestHostingService implements Pick<HostingMainService, 'status' | 'onDidChangeStatus' | 'setAccepting' | 'disconnect'> {
	private readonly changed = new Emitter<HostingStatus>();
	public readonly onDidChangeStatus = this.changed.event;
	public readonly requests: (readonly [string, unknown])[] = [];

	constructor(public status: HostingStatus) { }

	public get hasListeners(): boolean {
		return this.changed.hasListeners();
	}

	public setAccepting(accepting: boolean): Promise<void> {
		this.requests.push(['accept', accepting]);
		return Promise.resolve();
	}

	public disconnect(sessionId: string): Promise<void> {
		this.requests.push(['disconnect', sessionId]);
		return Promise.resolve();
	}

	public set(status: HostingStatus): void {
		this.status = status;
		this.changed.fire(status);
	}

	public dispose(): void {
		this.changed.dispose();
	}
}

suite('TrayMainService', () => {
	const disposables = new DisposableStore();
	let hosting: TestHostingService;
	setup(() => {
		hosting = new TestHostingService(ready);
		disposables.add(toDisposable(() => {
			hosting.dispose();
		}));
	});
	teardown(() => {
		disposables.clear();
	});
	function createTray(environment: TestTrayEnvironment): { readonly tray: TrayMainService; readonly icon: TestTrayIcon } {
		disposables.add(toDisposable(() => {
			environment.dispose();
		}));
		const instantiationService = disposables.add(new TestInstantiationService());
		instantiationService.stub(IHostingMainService, hosting);
		instantiationService.stub(ILogService, disposables.add(new NullLogService()));
		const tray = disposables.add(instantiationService.createInstance(TrayMainService, environment));
		tray.create();
		const icon = environment.icons[0];
		return { tray, icon };
	}
	function windowRequests(tray: TrayMainService): () => number {
		let count = 0;
		disposables.add(tray.onDidRequestWindow(() => count++));
		return () => count;
	}
	test('the application has one tray icon', () => {
		const environment = new TestTrayEnvironment('win32');
		const { tray } = createTray(environment);
		tray.create();
		assert.strictEqual(environment.icons.length, 1);
	});
	test('the icon shows the hosting state in the panel\'s artwork', () => {
		const { icon } = createTray(new TestTrayEnvironment('win32'));
		assert.deepStrictEqual(icon.images, ['idle-dark.png']);
	});
	test('the tooltip names the product and the status', () => {
		const { icon } = createTray(new TestTrayEnvironment('win32'));
		assert.strictEqual(icon.tooltip, `${product.name} – Ready for paired devices`);
	});
	test('a status change shows the new state', () => {
		const { icon } = createTray(new TestTrayEnvironment('win32'));
		hosting.set(connected);
		assert.strictEqual(icon.images.at(-1), 'active-dark.png');
		assert.strictEqual(icon.tooltip, `${product.name} – 2 devices connected`);
	});
	test('Disconnect ends that device\'s session', () => {
		hosting.status = connected;
		const { icon } = createTray(new TestTrayEnvironment('win32'));
		icon.choose('Disconnect Work laptop');
		assert.deepStrictEqual(hosting.requests, [['disconnect', '7']]);
	});
	test('Stop Accepting Connections pauses hosting', () => {
		const { icon } = createTray(new TestTrayEnvironment('win32'));
		icon.choose('Stop Accepting Connections');
		assert.deepStrictEqual(hosting.requests, [['accept', false]]);
	});
	test('Accept Connections resumes hosting', () => {
		hosting.status = paused;
		const { icon } = createTray(new TestTrayEnvironment('win32'));
		icon.choose('Accept Connections');
		assert.deepStrictEqual(hosting.requests, [['accept', true]]);
	});
	test('Open asks for the window', () => {
		const { tray, icon } = createTray(new TestTrayEnvironment('win32'));
		const requested = windowRequests(tray);
		icon.choose(`Open ${product.name}`);
		assert.strictEqual(requested(), 1);
	});
	test('Quit quits the application', () => {
		const environment = new TestTrayEnvironment('win32');
		const { icon } = createTray(environment);
		icon.choose(`Quit ${product.name}`);
		assert.strictEqual(environment.quits, 1);
	});
	test('an unchanged status rebuilds no menu and reloads no artwork', () => {
		hosting.status = connected;
		const { icon } = createTray(new TestTrayEnvironment('win32'));
		hosting.set({ ...connected });
		assert.strictEqual(icon.menus, 1);
		assert.strictEqual(icon.images.length, 1);
	});
	test('a theme update that keeps the panel\'s colour reloads no artwork', () => {
		const environment = new TestTrayEnvironment('win32');
		const { icon } = createTray(environment);
		environment.updateTheme(true);
		assert.strictEqual(icon.images.length, 1);
	});
	test('a Windows taskbar turning light loads the light artwork', () => {
		const environment = new TestTrayEnvironment('win32');
		const { icon } = createTray(environment);
		environment.updateTheme(false);
		assert.deepStrictEqual(icon.images, ['idle-dark.png', 'idle-light.png']);
	});
	for (const platform of ['win32', 'linux'] as const) {
		test(`clicking the icon on ${platform} asks for the window`, () => {
			const { tray, icon } = createTray(new TestTrayEnvironment(platform));
			const requested = windowRequests(tray);
			icon.click();
			assert.strictEqual(requested(), 1);
		});
	}
	test('clicking the icon on macOS opens its menu, not the window', () => {
		const { icon } = createTray(new TestTrayEnvironment('darwin'));
		assert.strictEqual(icon.listensForClicks, false);
	});
	test('disposing the service removes the icon', () => {
		const { tray, icon } = createTray(new TestTrayEnvironment('win32'));
		tray.dispose();
		assert.strictEqual(icon.destroyed, true);
	});
	test('a disposed tray stops listening to hosting', () => {
		const { tray } = createTray(new TestTrayEnvironment('win32'));
		tray.dispose();
		assert.strictEqual(hosting.hasListeners, false);
	});
	test('a disposed tray stops listening to the theme', () => {
		const environment = new TestTrayEnvironment('win32');
		const { tray } = createTray(environment);
		tray.dispose();
		assert.strictEqual(environment.listensForTheme, false);
	});
	ensureNoDisposablesAreLeakedInTestSuite();
});
