// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { release } from 'node:os';
import { join } from 'node:path';
import { nativeTheme, screen, type BrowserWindow } from 'electron';
import Mocha from 'mocha';
import { isMacintosh } from '../../../../../src/base/common/platform.js';
import { registerDevelopmentWindow } from '../../../../../src/code/electron-main/development.js';
import { record } from '../../../../../src/platform/engine/common/engine.js';
import { getWorkbenchTitlebarHeight } from '../../../../../src/platform/window/common/titlebar.js';
import { getMacOSWindowControlsPosition } from '../../../../../src/platform/window/common/window.js';
import { resolveWindowMaterial } from '../../../../../src/platform/window/common/windowMaterial.js';
import type { Automation } from '../../../../automation/src/automation.js';
import { restoresWindow, setNativeTheme } from '../../utils.js';

/** Resolves when `window` emits `event`, or fails after ten seconds. */
function windowEvent(window: BrowserWindow, event: 'minimize' | 'restore' | 'enter-full-screen' | 'leave-full-screen'): Promise<void> {
	return new Promise((resolve, reject) => {
		const timer = setTimeout(() => {
			window.removeListener(event as 'minimize', done);
			reject(new Error(`The window did not ${event}`));
		}, 10000);
		function done(): void {
			clearTimeout(timer);
			resolve();
		}
		window.once(event as 'minimize', done);
	});
}

/**
 * Resolves once `window.isMaximized()` is `maximized`. macOS zooms with an
 * animation and sends no `maximize` event when zooming changes nothing, so
 * the state is read rather than an event awaited.
 */
async function maximizedState(window: BrowserWindow, maximized: boolean): Promise<void> {
	const deadline = Date.now() + 10000;
	while (window.isMaximized() !== maximized) {
		if (Date.now() > deadline) {
			throw new Error(`The window did not ${maximized ? 'maximize' : 'restore from maximized'}`);
		}
		await new Promise(resolve => setTimeout(resolve, 50));
	}
}

const DARK_CONTENT = 'rgb(33, 33, 35)';

const LIGHT_CONTENT = 'rgb(255, 255, 255)';

/** The solid titlebar, where the window has no material: each platform's own chrome colour (colors.css). */
const SOLID_CHROME: Record<string, { readonly light: string; readonly dark: string }> = {
	darwin: { light: 'rgb(236, 236, 239)', dark: 'rgb(45, 45, 48)' },
	win32: { light: 'rgb(243, 243, 243)', dark: 'rgb(43, 43, 43)' },
	linux: { light: 'rgb(235, 235, 237)', dark: 'rgb(46, 46, 50)' },
};

/** Home's solid panel, where the window has no material: each platform's own sidebar colour (colors.css). */
const SOLID_PANEL: Record<string, { readonly light: string; readonly dark: string }> = {
	darwin: { light: 'rgb(241, 241, 243)', dark: 'rgb(39, 39, 42)' },
	win32: { light: 'rgb(243, 243, 243)', dark: 'rgb(43, 43, 43)' },
	linux: { light: 'rgb(235, 235, 237)', dark: 'rgb(46, 46, 50)' },
};

/** How far apart two `#rrggbb` colours are, by their most different channel. */
function distance(first: string, second: string): number {
	const channels = (color: string) => [1, 3, 5].map(index => Number.parseInt(color.slice(index, index + 2), 16));
	const [a, b] = [channels(first), channels(second)];
	return Math.max(...a.map((value, index) => Math.abs(value - b[index])));
}

/** `rgb(r, g, b)` as `#rrggbb`. */
function hex(color: string): string {
	const channels = /^rgb\((\d+), (\d+), (\d+)\)$/.exec(color);
	assert.ok(channels, `${color} is not an opaque colour`);
	return `#${channels.slice(1).map(value => Number(value).toString(16).padStart(2, '0')).join('')}`;
}

/** The system composes a capture in the display's colour space, a few steps from the page's sRGB. */
const COMPOSED = 6;

/** What the titlebar and sidebar paint over `material` in `theme`, where `opaque` is the titlebar's colour without one. */
function chrome(material: string | undefined, theme: 'light' | 'dark', opaque: string): string {
	if (material === 'none') {
		return opaque;
	}
	if (material === 'vibrancy') {
		return theme === 'dark' ? 'rgba(32, 33, 36, 0.22)' : 'rgba(248, 248, 250, 0.58)';
	}
	return 'rgba(0, 0, 0, 0)';
}

/** The window, its Home page and its settings, as the person using it meets them. */
export function setup(automation: Automation): void {
	const renderer = automation.driver.renderer;
	const nativeHost = async (method: string, ...args: unknown[]): Promise<unknown> =>
		renderer.callMainProcess('nativeHost', method, [await renderer.getWindowId(), ...args]);
	Mocha.describe('navigation', () => {
		Mocha.it('lists Devices and the settings sections, with no retired branding', async () => {
			assert.deepEqual((await automation.home.sidebarItems()).map(item => item.label), ['Devices', 'General', 'Connection', 'Access & Security', 'Remote Desktop', 'Keyboard & Mouse', 'Terminal', 'Audio & Camera', 'Recording', 'Power']);
			assert.equal(await automation.home.sidebarHeading(), 'Settings');
			assert.equal(await automation.home.hasRetiredChrome(), false, 'old branding or a status bar remains');
			assert(Math.abs(await automation.home.sidebarBottom() - await automation.workbench.viewportHeight()) < 1, 'the sidebar is not full height');
		});
		Mocha.it('opens each section, marked as the one shown, its icon filled and the others outlined', async () => {
			await automation.home.markPage('devices');
			for (const item of (await automation.home.sidebarItems()).filter(item => item.id !== 'devices')) {
				await automation.home.open(item.id);
				assert.equal(await automation.workbench.heading(), item.label, `the sidebar did not open ${item.label}`);
				assert.deepEqual(await automation.home.selected(), [item.id], 'the selection is ambiguous');
				assert.equal((await automation.home.iconOf(item.id))?.variant, 'filled', `${item.label}'s icon is not filled`);
				assert((await automation.home.unselectedIconVariants()).every(variant => variant === 'outlined'), 'an unselected icon is not outlined');
				await automation.home.waitForNoDeviceActions();
				await automation.home.markPage(item.id);
			}
		});
		Mocha.it('keeps each page, settings and Devices alike, when its section is opened again', async () => {
			for (const item of await automation.home.sidebarItems()) {
				await automation.home.open(item.id);
				assert.equal(await automation.home.showsMarkedPage(item.id), true, `${item.label} was built again`);
			}
			await automation.home.open('devices');
		});
		Mocha.it('offers pairing on the Devices page and none in the titlebar', async () => {
			assert.equal(await automation.home.deviceActionCount(), 0, 'pairing actions are in the titlebar');
			assert.equal(await automation.home.offersPairing(), true, 'pairing is not on the Devices page');
			assert.deepEqual(await automation.home.selected(), ['devices']);
			assert.deepEqual(await automation.home.titlebarActions(), []);
			assert.equal((await automation.home.iconOf('devices'))?.icon, 'devices', 'wrong Devices artwork');
			assert.equal((await automation.home.iconOf('desktop'))?.icon, 'remoteDesktop', 'wrong Remote Desktop artwork');
			await automation.screenshot('navigation');
		});
	});
	Mocha.describe('window', () => {
		const height = getWorkbenchTitlebarHeight(release());
		restoresWindow(automation);
		Mocha.it('draws a titlebar of the platform\'s height that drags the window, without its own controls or menu', async () => {
			assert.deepEqual(await automation.titlebar.presentation(), { height, drag: 'drag', unwanted: 0 });
		});
		Mocha.it('sits the selected tab on the titlebar\'s floor in its page\'s colour, draggable around it but not itself', async () => {
			// Home has no titlebar actions; the titlebar tabs unit suite checks an action's drag region.
			const page = (await automation.titlebar.surfaces()).content;
			assert.deepEqual(await automation.titlebar.selectedTab(), { height: 35, topGap: height - 35, bottomGap: 0, radius: '12px', fill: page, strip: 'drag', container: 'drag', tab: 'no-drag', close: false, action: null });
		});
		Mocha.it('gets the native material this system should draw, under a titlebar and Home panel that let it through', async () => {
			// Liquid Glass on macOS 26 and later, so a silent fallback to vibrancy fails here.
			const { prefersReducedTransparency, shouldUseHighContrastColors } = nativeTheme;
			const material = resolveWindowMaterial(process.platform, release(), prefersReducedTransparency, shouldUseHighContrastColors);
			assert.equal(await automation.workbench.material(), material, 'the window did not get its native material');
			const surfaces = await automation.titlebar.surfaces();
			assert.notEqual(surfaces.content, 'rgba(0, 0, 0, 0)', 'view content became transparent');
			if (material !== 'none') {
				const expected = chrome(material, nativeTheme.shouldUseDarkColors ? 'dark' : 'light', '');
				assert.equal(surfaces.titlebar, expected);
				assert.equal(surfaces.sidebar, expected);
				// Electron's getter returns RGB and omits the configured alpha channel.
				assert.equal(automation.window.getBackgroundColor(), '#000000');
			}
		});
		Mocha.it('ends the toolbar where the native controls\' reservation begins', async () => {
			assert(Math.abs(await automation.titlebar.toolbarGap()) < 0.1, 'the toolbar is inset beyond the native control reservation');
			if (isMacintosh) {
				assert.deepEqual(automation.window.getWindowButtonPosition(), getMacOSWindowControlsPosition(height, release()));
			}
		});
		for (const zoom of [1.25, 1]) {
			Mocha.it(`keeps the titlebar and its tabs their size at ${String(zoom * 100)}% zoom`, async () => {
				automation.window.webContents.setZoomFactor(zoom);
				await automation.titlebar.waitForHeight(height / zoom);
				assert(Math.abs(await automation.titlebar.tabHeight() - 35 / zoom) < 0.1, 'titlebar zoom stretched the tabs');
			});
		}
		Mocha.it('refuses window actions on another window, out-of-range controls and unknown fields', async () => {
			const windowId = await renderer.getWindowId();
			const calls: [string, unknown[]][] = [
				['getWindowState', [windowId + 1000]],
				['updateWindowControls', [windowId, { height: -1 }]],
				['focusWindow', [windowId, { targetWindowId: windowId + 1000 }]],
				['updateWindowControls', [windowId, { arbitrary: true }]],
				['openExternal', [windowId, 'https://example.com/']],
			];
			for (const [method, args] of calls) {
				await assert.rejects(renderer.callMainProcess('nativeHost', method, args), `${method} ${JSON.stringify(args)} was accepted`);
			}
		});
		Mocha.it('hears a real window state change, and stops listening after', async () => {
			await renderer.listenForAlwaysOnTop();
			automation.window.setAlwaysOnTop(true);
			const state = await renderer.alwaysOnTop();
			automation.window.setAlwaysOnTop(false);
			assert(record(state) && state.windowId === automation.window.id && state.alwaysOnTop === true);
		});
		for (const theme of ['dark', 'light'] as const) {
			Mocha.it(`follows the system into the ${theme} theme`, async () => {
				await setNativeTheme(automation, theme);
				await automation.titlebar.waitForBackground(chrome(await automation.workbench.material(), theme, SOLID_CHROME[process.platform][theme]));
				await automation.workbench.waitForContentBackground(theme === 'dark' ? DARK_CONTENT : LIGHT_CONTENT);
			});
		}
		Mocha.it('focuses, maximizes and restores through its own actions', async () => {
			automation.window.show();
			// Half the work area, so maximizing changes the window whatever size earlier tests left.
			const area = screen.getDisplayMatching(automation.window.getBounds()).workArea;
			automation.window.setBounds({ x: area.x, y: area.y, width: Math.floor(area.width / 2), height: Math.floor(area.height / 2) });
			assert.equal(automation.window.isMaximized(), false, 'a window half the work area counts as maximized');
			await nativeHost('focusWindow');
			await nativeHost('maximizeWindow');
			await maximizedState(automation.window, true);
			await nativeHost('unmaximizeWindow');
			await maximizedState(automation.window, false);
		});
		for (const fullscreen of [true, false]) {
			Mocha.it(`${fullscreen ? 'hides' : 'shows'} the titlebar as it ${fullscreen ? 'enters' : 'leaves'} full screen`, async () => {
				// Full screen changes asynchronously on macOS; the page is checked after the native event.
				const transitioned = windowEvent(automation.window, fullscreen ? 'enter-full-screen' : 'leave-full-screen');
				await nativeHost('setFullScreen', fullscreen);
				await transitioned;
				await automation.titlebar.waitForHidden(fullscreen);
			});
		}
		Mocha.it('keeps Home as a permanent tab with the sidebar in its panel', async () => {
			assert.deepEqual(await automation.titlebar.homeTab(), { label: 'Home', sidebarInPanel: true, closable: false, draggable: false });
		});
		Mocha.it('minimizes and is restored by focusing it', async () => {
			// Minimizing and restoring animate on macOS. Each is awaited: focusing
			// a window still minimizing restores nothing, and the scenarios after
			// would run in a minimized window, which gets no animation frames.
			const minimized = windowEvent(automation.window, 'minimize');
			await nativeHost('minimizeWindow');
			await minimized;
			const restored = windowEvent(automation.window, 'restore');
			await nativeHost('focusWindow');
			await restored;
			assert.equal(automation.window.isMinimized(), false, 'focus left the window minimized');
			const state = await nativeHost('getWindowState');
			assert(record(state) && state.windowId === automation.window.id && state.fullscreen === false && state.alwaysOnTop === false);
			await automation.screenshot('window');
		});
	});
	Mocha.describe('window-theme', () => {
		restoresWindow(automation);
		Mocha.after(() => automation.settings.setTheme('system'));
		// The system composes, and so captures, only a window on screen.
		let shown = false;
		Mocha.before(async () => {
			shown = automation.window.isVisible();
			automation.window.show();
			automation.window.setSize(1120, 760);
			await automation.home.open('devices');
		});
		Mocha.after(() => {
			if (!shown) {
				automation.window.hide();
			}
		});
		Mocha.it('insets Home\'s sidebar as a rounded panel in Home\'s page, below the tabs', async () => {
			const panel = await automation.home.panel();
			const { height } = await automation.titlebar.bounds();
			const viewport = await automation.workbench.viewportHeight();
			// Within a pixel: the page's height is whole pixels, its layout fractional at Windows' 125%.
			const inset = (value: number) => Math.abs(value - 8) < 1;
			assert.deepEqual({ left: inset(panel.left), top: inset(panel.top - height), bottom: inset(viewport - panel.bottom), radius: panel.radius }, { left: true, top: true, bottom: true, radius: '10px' }, `the panel is at ${JSON.stringify(panel)} in a ${String(viewport)} px page below a ${String(height)} px titlebar`);
		});
		for (const theme of ['light', 'dark'] as const) {
			Mocha.it(`draws the ${theme} chrome and panel in this system\'s material, or its own solid colours, beside an opaque page`, async () => {
				await automation.settings.setTheme(theme);
				await automation.settings.open('general');
				await automation.home.open('devices');
				await automation.driver.renderer.whenRendered();
				const material = await automation.workbench.material();
				const surfaces = await automation.titlebar.surfaces();
				const panel = await automation.home.panel();
				if (material === 'none' && !nativeTheme.shouldUseHighContrastColors) {
					assert.equal(panel.background, SOLID_PANEL[process.platform][theme], 'the panel is not this platform\'s sidebar colour');
				} else if (material !== 'none') {
					assert.equal(panel.background, chrome(material, theme, ''), 'the panel hides the material');
				}
				automation.window.focus();
				await automation.driver.renderer.whenRendered();
				const fill = await automation.titlebar.selectedTabFill();
				const bar = await automation.titlebar.bounds();
				const page = await automation.home.page();
				const points = {
					strip: { x: bar.width * 0.6, y: bar.height / 2 },
					tab: { x: fill.left + 4, y: (fill.top + fill.bottom) / 2 },
					panel: { x: (panel.left + panel.right) / 2, y: panel.bottom - 16 },
					gutter: { x: panel.left / 2, y: panel.bottom - 16 },
					// Clear of the corners, where a system can draw over every window (Windows' activation notice).
					page: { x: (page.left + page.right) / 2, y: page.bottom - 40 },
				};
				const colors = await automation.captureNative(`window-theme-${theme}`, points);
				await automation.screenshot(`window-theme-${theme}`);
				if (!colors) {
					// The system gave no picture of the window; the page capture above and the DOM checks still ran.
					return;
				}
				const canvas = hex(surfaces.content);
				for (const point of ['tab', 'gutter', 'page'] as const) {
					assert.ok(distance(colors[point], canvas) <= COMPOSED, `the ${point} is ${colors[point]}, not the page's ${canvas}`);
				}
				if (material === 'none') {
					assert.ok(distance(colors.panel, hex(panel.background)) <= COMPOSED, `the solid panel is ${colors.panel}, not ${panel.background}`);
				} else if (material !== 'mica') {
					assert.ok(distance(colors.panel, colors.page) >= COMPOSED, `the ${String(material)} panel, ${colors.panel}, is lost against the page, ${colors.page}`);
				}
				// Mica takes the desktop's colours, so how far it stands from the page depends on the
				// wallpaper; dark Mica over a plain one is the page's colour. Recorded, not judged.
				automation.window.blur();
				await automation.driver.renderer.whenRendered();
				// Unfocused, the material greys itself; recorded for comparison, not judged.
				await automation.captureNative(`window-theme-${theme}-unfocused`, points);
			});
		}
	});
	Mocha.describe('appearance', () => {
		restoresWindow(automation);
		Mocha.after(() => automation.settings.setTheme('system'));
		Mocha.it('applies a saved dark theme over the same material and persists it', async () => {
			const material = await automation.workbench.material();
			await automation.settings.setTheme('dark');
			await automation.workbench.waitForContentBackground(DARK_CONTENT);
			assert.equal(await automation.workbench.material(), material);
			assert.deepEqual(JSON.parse(readFileSync(join(automation.application.profile, 'settings.json'), 'utf8')), { 'appearance.theme': 'dark' });
			assert.equal(nativeTheme.themeSource, 'dark');
		});
		Mocha.it('refuses a theme that does not exist', async () => {
			await assert.rejects(automation.settings.setTheme('invalid'), 'an invalid preference was accepted');
		});
		Mocha.it('fits the theme picker on General in a small window', async () => {
			automation.window.setSize(760, 520);
			await automation.settings.open('general');
			assert.deepEqual(await automation.settings.themePicker(), { choices: 3, selected: 'dark', overflows: false, contained: true, headingVisible: true, inGeneral: true });
			await automation.screenshot('appearance');
		});
		Mocha.it('returns to the system theme without an error', async () => {
			await automation.settings.setTheme('system');
			await automation.driver.renderer.whenRendered();
			assert.equal(await automation.workbench.noticeHidden(), true, 'the native theme update raised an application error');
		});
	});
	Mocha.describe('settings-layout', () => {
		restoresWindow(automation);
		Mocha.after(() => automation.settings.setTheme('system'));
		const categories = ['general', 'access', 'desktop', 'input', 'terminal', 'media', 'recording', 'power', 'connection'];
		// The layout does not depend on the theme: it is checked in the light theme, and the dark one is only pictured.
		for (const width of [1200, 760]) {
			for (const category of categories) {
				Mocha.it(`lays out ${category} at ${String(width)} px, each fact once and no button to save`, async () => {
					await automation.settings.setTheme('light');
					automation.window.setSize(width, 820);
					await automation.settings.open(category);
					const page = await automation.settings.facts();
					assert.equal(page.overflows, false, `${category} overflowed at ${String(width)} px`);
					assert.equal(page.submits, false, `${category} still saves through a button`);
					assert.equal(page.refreshes, false, `${category} offers a Refresh button`);
					// Each fact is said once, in its row or section; pages carry no closing note.
					assert.equal(page.note, '', `${category} has a page note`);
					assert.equal(page.legacyRows, 0, `${category} uses a retired row layout`);
					if (category === 'access') {
						assert.equal(page.text.includes('Device access'), false);
					}
					if (category === 'desktop') {
						assert.equal(page.text.includes('Session overrides'), false);
					}
					await automation.settings.waitUntilSettled();
					await automation.screenshot(`settings-${category}-light-${String(width)}`);
				});
			}
		}
		for (const width of [1200, 760]) {
			for (const category of categories) {
				Mocha.it(`pictures ${category} in the dark theme at ${String(width)} px`, async () => {
					await automation.settings.setTheme('dark');
					automation.window.setSize(width, 820);
					await automation.settings.open(category);
					await automation.settings.waitUntilSettled();
					await automation.screenshot(`settings-${category}-dark-${String(width)}`);
				});
			}
		}
		Mocha.it('shows the welcome across its whole tab, at any width', async () => {
			await automation.settings.open('general');
			await automation.onboarding.open();
			for (const width of [1200, 760]) {
				automation.window.setSize(width, 820);
				const { welcome, tab } = await automation.onboarding.widths(automation.window.getContentSize()[0]);
				assert.ok(Math.abs(welcome - tab) <= 1, `the welcome is ${String(welcome)} px in a ${String(tab)} px tab at ${String(width)} px`);
			}
			await automation.screenshot('welcome');
			await automation.onboarding.skip();
		});
		Mocha.it('keeps each welcome step the column\'s width, each permission\'s name beside a button of its own width', async () => {
			automation.window.setSize(760, 820);
			await automation.settings.open('general');
			await automation.onboarding.open();
			const column = await automation.onboarding.stepWidth();
			const shown = await automation.onboarding.openPermissions();
			// The first step's text fills the column; a step with less in it keeps that width.
			const width = await automation.onboarding.stepWidth();
			assert.ok(Math.abs(width - column) < 1, `the step is ${String(width)} px in a ${String(column)} px column`);
			if (shown) {
				// Only a permission still to grant has a button; one already granted shows its status alone.
				for (const { row, text, button } of await automation.onboarding.permissionRows()) {
					assert.ok(text > button, `a permission's name is ${String(text)} px beside a ${String(button)} px button in a ${String(row)} px row`);
				}
				await automation.screenshot('welcome-permissions');
			}
			await automation.onboarding.skip();
		});
	});
	Mocha.describe('reload', () => {
		let endpointId = '';
		Mocha.before(async () => {
			const snapshot = await renderer.execute({ type: 'snapshot' });
			assert(snapshot.type === 'snapshot');
			endpointId = snapshot.snapshot.endpoint_id;
			const previous = process.env.VNILINK_DEV;
			process.env.VNILINK_DEV = '1';
			try {
				registerDevelopmentWindow(automation.window);
			} finally {
				if (previous === undefined) {
					delete process.env.VNILINK_DEV;
				} else {
					process.env.VNILINK_DEV = previous;
				}
			}
		});
		for (const reload of [1, 2, 3]) {
			Mocha.it(`reloads the window on the development shortcut, keeping its engine (${String(reload)} of 3)`, async () => {
				const loaded = new Promise<void>((resolve, reject) => {
					const timer = setTimeout(() => {
						automation.window.webContents.removeListener('did-finish-load', done);
						reject(new Error('Development reload timed out'));
					}, 5000);
					function done(): void {
						clearTimeout(timer);
						resolve();
					}
					automation.window.webContents.once('did-finish-load', done);
				});
				let prevented = false;
				automation.window.webContents.emit('before-input-event', {
					preventDefault: () => {
						prevented = true;
					}
				}, { type: 'keyDown', key: 'r', meta: isMacintosh, control: !isMacintosh, alt: false, shift: false });
				await loaded;
				assert.equal(prevented, true);
				await automation.driver.ready();
				const snapshot = await renderer.execute({ type: 'snapshot' });
				assert(snapshot.type === 'snapshot' && snapshot.snapshot.endpoint_id === endpointId, 'reload replaced the native engine');
			});
		}
		Mocha.after(() => automation.screenshot('reload'));
	});
}
