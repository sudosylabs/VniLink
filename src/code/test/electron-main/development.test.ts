// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'assert';
import type { BrowserWindow, Event, Input } from 'electron';
import { isMacintosh } from '../../../base/common/platform.js';
import { ensureNoDisposablesAreLeakedInTestSuite } from '../../../base/test/common/utils.js';
import { registerDevelopmentWindow } from '../../electron-main/development.js';

type InputListener = (event: Event, input: Input) => void;

/** A window that records its `before-input-event` listener and what the shortcuts did. */
class TestWindow {
	public listener: InputListener | undefined;
	public reloaded = 0;
	public toggled = 0;
	public prevented = 0;

	/** This window as Electron's: only `webContents.on`, `reloadIgnoringCache` and `toggleDevTools` are used. */
	public asBrowserWindow(): BrowserWindow {
		const webContents = {
			on: (_name: string, listener: InputListener) => {
				this.listener = listener;
			},
			reloadIgnoringCache: () => {
				this.reloaded++;
			},
			toggleDevTools: () => {
				this.toggled++;
			},
		};
		const window = { webContents };
		return window as BrowserWindow;
	}

	/** Delivers a key press to the registered listener. */
	public press(input: Partial<Input>): void {
		assert.ok(this.listener, 'no input listener was registered');
		const event = {
			preventDefault: () => {
				this.prevented++;
			},
		};
		this.listener(event as Event, { ...shortcut, ...input });
	}
}

/** The platform modifier with R: Cmd+R on macOS, Ctrl+R elsewhere. */
const shortcut: Input = { type: 'keyDown', key: 'r', code: 'KeyR', isAutoRepeat: false, isComposing: false, location: 0, modifiers: [], meta: isMacintosh, control: !isMacintosh, alt: false, shift: false };

/** Toggle Developer Tools: Cmd+Option+I on macOS, which reports the key as `ˆ`, and Ctrl+Shift+I elsewhere. */
const devtools: Partial<Input> = isMacintosh ? { key: 'ˆ', code: 'KeyI', alt: true } : { key: 'I', code: 'KeyI', shift: true };

/** Sets what Electron reports for a launch from the source checkout, which its types declare read-only. */
function setDefaultApp(value: boolean | undefined): void {
	(process as { defaultApp?: boolean }).defaultApp = value;
}

suite('registerDevelopmentWindow', () => {
	ensureNoDisposablesAreLeakedInTestSuite();
	let development: string | undefined;
	let defaultApp: boolean | undefined;
	let window: TestWindow;
	setup(() => {
		development = process.env.VNILINK_DEV;
		defaultApp = process.defaultApp;
		window = new TestWindow();
	});
	teardown(() => {
		setDefaultApp(defaultApp);
		if (development === undefined) {
			delete process.env.VNILINK_DEV;
		} else {
			process.env.VNILINK_DEV = development;
		}
	});

	/** Registers the shortcuts as a development launch from the source checkout does. */
	function registerInDevelopment(): void {
		process.env.VNILINK_DEV = '1';
		setDefaultApp(true);
		registerDevelopmentWindow(window.asBrowserWindow());
	}
	test('registers no shortcuts in a checkout launch that is not a development launch', () => {
		delete process.env.VNILINK_DEV;
		setDefaultApp(true);
		registerDevelopmentWindow(window.asBrowserWindow());
		assert.strictEqual(window.listener, undefined);
	});
	test('registers no shortcuts in an installed build', () => {
		process.env.VNILINK_DEV = '1';
		setDefaultApp(undefined);
		registerDevelopmentWindow(window.asBrowserWindow());
		assert.strictEqual(window.listener, undefined);
	});
	test('R without the platform modifier does not reload', () => {
		registerInDevelopment();
		window.press({ meta: false, control: false });
		assert.deepStrictEqual([window.reloaded, window.prevented], [0, 0]);
	});
	test('the platform modifier and R with Alt does not reload', () => {
		registerInDevelopment();
		window.press({ alt: true });
		assert.deepStrictEqual([window.reloaded, window.prevented], [0, 0]);
	});
	test('the platform modifier and R reloads the window', () => {
		registerInDevelopment();
		window.press({});
		assert.strictEqual(window.reloaded, 1);
	});
	test('the reload shortcut is kept from the page', () => {
		registerInDevelopment();
		window.press({});
		assert.strictEqual(window.prevented, 1);
	});
	test('the developer tools shortcut toggles developer tools', () => {
		registerInDevelopment();
		window.press(devtools);
		assert.strictEqual(window.toggled, 1);
	});
	test('the developer tools shortcut is kept from the page', () => {
		registerInDevelopment();
		window.press(devtools);
		assert.strictEqual(window.prevented, 1);
	});
});
