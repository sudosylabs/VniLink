// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'assert';
import '../../../../media/workbench.css';
import '../../terminal.contribution.js';
import { $ } from '../../../../../base/browser/dom.js';
import { Emitter, Event } from '../../../../../base/common/event.js';
import { Disposable } from '../../../../../base/common/lifecycle.js';
import { isMacintosh } from '../../../../../base/common/platform.js';
import { animationFrames, until } from '../../../../../base/test/browser/waits.js';
import { ensureNoDisposablesAreLeakedInTestSuite } from '../../../../../base/test/common/utils.js';
import { IClipboardService } from '../../../../../platform/clipboard/common/clipboardService.js';
import { allowedStyleHashes } from '../../../../../platform/window/common/contentSecurityPolicy.js';
import { TestInstantiationService } from '../../../../../platform/instantiation/test/common/instantiationServiceMock.js';
import type { IProcessDataEvent, ITerminalChildProcess, ITerminalLaunchError, TerminalExit } from '../../../../../platform/terminal/common/terminal.js';
import { IThemeService, type ITitlebarTheme } from '../../../../../platform/theme/common/themeService.js';
import type { DeviceSummary } from '../../../../services/devices/deviceService.js';
import { ITabService } from '../../../../services/tabs/tabService.js';
import { TabService } from '../../../../services/tabs/tabServiceImpl.js';
import { ITerminalService } from '../../../../services/terminal/terminalService.js';
import { TestClipboardService } from '../../../../test/common/workbenchTestServices.js';
import { TerminalView } from '../../terminalView.js';

/** A scripted shell: keeps what the view sends and lets the test answer. */
class TestPty implements ITerminalChildProcess {
	private readonly data = new Emitter<IProcessDataEvent>();
	public readonly onProcessData = this.data.event;
	public readonly onProcessReady = Event.None;
	private readonly exit = new Emitter<TerminalExit>();
	public readonly onProcessExit = this.exit.event;
	public readonly inputs: string[] = [];
	public readonly sizes: [number, number][] = [];
	public started = 0;
	public shutdowns = 0;
	public acknowledged = 0;

	constructor(public readonly cols: number, public readonly rows: number, private readonly launchError?: ITerminalLaunchError) { }

	public start(): Promise<ITerminalLaunchError | undefined> {
		this.started++;
		return Promise.resolve(this.launchError);
	}

	public input(data: string): void {
		this.inputs.push(data);
	}

	public processBinary(data: string): void {
		this.inputs.push(data);
	}

	public resize(cols: number, rows: number): void {
		this.sizes.push([cols, rows]);
	}

	public shutdown(): void {
		this.shutdowns++;
	}

	public dispose(): void {
		this.shutdown();
		this.data.dispose();
		this.exit.dispose();
	}

	/** Sends `text` as the shell's output; resolves once the view acknowledges it. */
	public output(text: string): Promise<void> {
		return new Promise(resolve => {
			this.data.fire({
				data: new TextEncoder().encode(text), acknowledge: () => {
					this.acknowledged++;
					resolve();
				},
			});
		});
	}

	public end(exit: TerminalExit): void {
		this.exit.fire(exit);
	}
}

interface XtermBufferForTest {
	readonly length: number;
	getLine(index: number): { readonly isWrapped: boolean; translateToString(trim: boolean): string } | undefined;
}

interface XtermForTest {
	input(data: string, wasUserInput?: boolean): void;
	selectAll(): void;
	readonly options: { readonly theme?: Readonly<Record<string, string>> };
	readonly buffer: { readonly active: XtermBufferForTest };
}

const device: DeviceSummary = { id: 'f'.repeat(64), name: 'Studio Mac', os: 'macos', presence: 'connected', hostname: 'studio.local', direct: 1, relays: 0 };

/** Whether this window can draw with WebGL 2; the Linux CI container cannot. */
const webgl = $<HTMLCanvasElement>('canvas').getContext('webgl2') !== null;

/** The terminal tab over a scripted shell service, mounted at 640×360. */
class TerminalFixture extends Disposable {
	public readonly ptys: TestPty[] = [];
	public readonly launchErrors: (ITerminalLaunchError | undefined)[] = [];
	public readonly clipboard = new TestClipboardService();
	public readonly themeChanged = this._register(new Emitter<ITitlebarTheme>());
	public readonly tabService = this._register(new TabService());
	public readonly view: TerminalView;
	private readonly host = $('div');

	constructor() {
		super();
		const instantiationService = this._register(new TestInstantiationService());
		instantiationService.stub(ITabService, this.tabService);
		instantiationService.stub(IThemeService, { onDidColorThemeChange: this.themeChanged.event, getColorTheme: () => ({ background: '#000000', foreground: '#ffffff', inactiveForeground: '#888888' }) });
		this.clipboard.readText = () => Promise.resolve('pasted text');
		instantiationService.stub(IClipboardService, this.clipboard);
		instantiationService.stub(ITerminalService, {
			createProcess: (_id: string, cols: number, rows: number) => {
				const pty = new TestPty(cols, rows, this.launchErrors.shift());
				this.ptys.push(pty);
				return pty;
			},
		});
		this.host.className = 'view-host';
		this.host.style.cssText = 'position:fixed;left:240px;top:80px;width:640px;height:360px;z-index:99;display:flex;';
		document.body.append(this.host);
		this.view = this._register(instantiationService.createInstance(TerminalView, device));
	}

	/** Shows the tab; the view starts its shell once it has a size. */
	public open(): void {
		this.host.append(this.view.element);
		this.tabService.open(this.view);
	}

	/** Shows the tab and waits for its first shell to start. */
	public async start(): Promise<TestPty> {
		this.open();
		await until(() => this.ptys.length === 1 && this.ptys[0].started === 1, 'The shell did not start once the tab had a size');
		return this.ptys[0];
	}

	/** The view's xterm, reached for assertions only. */
	public xterm(): XtermForTest {
		const xterm: unknown = Reflect.get(this.view, 'xterm');
		const raw: unknown = typeof xterm === 'object' && xterm !== null ? Reflect.get(xterm, 'raw') : undefined;
		assert.ok(typeof raw === 'object' && raw !== null, 'The view has no terminal');
		return raw as XtermForTest;
	}

	/** What the terminal shows, a wrapped row joined to the line it continues. */
	public screenText(): string {
		const buffer = this.xterm().buffer.active;
		let text = '';
		for (let index = 0; index < buffer.length; index++) {
			const line = buffer.getLine(index);
			const continued = buffer.getLine(index + 1)?.isWrapped === true;
			text += (line?.isWrapped ? '' : '\n') + (line?.translateToString(!continued) ?? '');
		}
		return text;
	}

	public async waitForText(text: string): Promise<void> {
		await until(() => this.screenText().includes(text), `The terminal does not show: ${text}`);
	}

	/** Presses Enter in the terminal, as typing it does. */
	public enter(): void {
		this.xterm().input('\r', true);
	}

	public override dispose(): void {
		super.dispose();
		this.host.remove();
	}
}

/** `test`, where this window can draw a terminal; skipped where it cannot, as such a terminal starts no shell. */
const drawn = webgl ? test : test.skip;

suite('TerminalView', () => {
	const disposables = ensureNoDisposablesAreLeakedInTestSuite();
	// The view lays the terminal out from its resize observer, and a layout that changes the
	// observed size makes the browser report, with a later frame, the notification it could not
	// deliver. That report is the browser's notice of a second layout pass, not a failure.
	let reportError: OnErrorEventHandler;
	suiteSetup(() => {
		reportError = window.onerror;
		window.onerror = (message, ...rest): unknown => typeof message === 'string' && message.startsWith('ResizeObserver loop') ? true : reportError?.call(window, message, ...rest);
	});
	suiteTeardown(async () => {
		await animationFrames();
		window.onerror = reportError;
	});
	let fixture: TerminalFixture;
	setup(() => {
		fixture = disposables.add(new TerminalFixture());
	});
	test('the tab is a region named for the computer', () => {
		assert.ok(fixture.view.id.startsWith(`terminal:${device.id}:`));
		assert.strictEqual(fixture.view.label, 'Studio Mac Terminal');
		assert.strictEqual(fixture.view.element.getAttribute('role'), 'region');
		assert.strictEqual(fixture.view.element.getAttribute('aria-label'), 'Terminal on Studio Mac');
	});
	test('a terminal that cannot draw says so, announced, and starts no shell', async () => {
		// eslint-disable-next-line @typescript-eslint/unbound-method
		const getContext = HTMLCanvasElement.prototype.getContext;
		HTMLCanvasElement.prototype.getContext = function (this: HTMLCanvasElement, kind: string, ...rest: unknown[]) {
			return kind === 'webgl2' ? null : Reflect.apply(getContext, this, [kind, ...rest]) as RenderingContext | null;
		} as typeof getContext;
		try {
			fixture.open();
			await until(() => fixture.view.element.querySelector('.terminal-notice') !== null, 'A terminal that cannot draw did not say so');
			await animationFrames();
		} finally {
			HTMLCanvasElement.prototype.getContext = getContext;
		}
		assert.strictEqual(fixture.view.element.querySelector('.terminal-notice')?.getAttribute('role'), 'alert');
		assert.strictEqual(fixture.ptys.length, 0);
	});
	drawn('the shell starts once, at the measured grid, drawn by WebGL', async () => {
		const pty = await fixture.start();
		assert.notStrictEqual(fixture.view.element.querySelector('.xterm-screen canvas'), null);
		// The measured grid, not xterm's 80×24 default.
		assert.ok(pty.cols !== 80 && pty.rows !== 24, `The shell started at ${String(pty.cols)}×${String(pty.rows)}`);
	});
	drawn('every style element xterm writes is one the window\'s policy admits, after a theme change too', async () => {
		await fixture.start();
		fixture.themeChanged.fire({ background: '#000000', foreground: '#ffffff', inactiveForeground: '#888888' });
		const styles = [...fixture.view.element.querySelectorAll('style')];
		assert.ok(styles.length > 0, 'xterm wrote no style element');
		for (const style of styles) {
			const digest = await crypto.subtle.digest('SHA-256', new TextEncoder().encode(style.textContent));
			const hash = `sha256-${btoa(String.fromCharCode(...new Uint8Array(digest)))}`;
			assert.ok(allowedStyleHashes.includes(hash), `The policy refuses this style element (${hash}): ${style.textContent}`);
		}
	});
	drawn('the terminal says it is connecting to the computer', async () => {
		await fixture.start();
		await fixture.waitForText('Connecting to Studio Mac…');
	});
	drawn('output reaches the terminal and is acknowledged once parsed', async () => {
		const pty = await fixture.start();
		await pty.output('hello \x1b[31mred\x1b[0m 你好\r\n');
		assert.strictEqual(pty.acknowledged, 1);
		assert.ok(fixture.screenText().includes('hello red 你好'));
	});
	drawn('typing reaches the shell', async () => {
		const pty = await fixture.start();
		fixture.xterm().input('ls\r', true);
		assert.deepStrictEqual(pty.inputs.join(''), 'ls\r');
	});
	suite('clipboard', () => {
		let pty: TestPty;
		let textarea: HTMLTextAreaElement;
		setup(async () => {
			if (!webgl) {
				return;
			}
			pty = await fixture.start();
			await pty.output('hello \x1b[31mred\x1b[0m 你好\r\n');
			const input = fixture.view.element.querySelector('textarea');
			assert.ok(input, 'xterm has no input element');
			textarea = input;
		});
		drawn('copy places the selection on the clipboard', async () => {
			fixture.xterm().selectAll();
			textarea.dispatchEvent(new KeyboardEvent('keydown', { ...(isMacintosh ? { key: 'c', metaKey: true } : { key: 'C', ctrlKey: true, shiftKey: true }), bubbles: true, cancelable: true }));
			await until(() => fixture.clipboard.written.some(text => text.includes('hello red 你好')), 'Copy did not reach the clipboard');
		});
		drawn('paste sends the clipboard to the shell', async () => {
			textarea.dispatchEvent(new KeyboardEvent('keydown', { ...(isMacintosh ? { key: 'v', metaKey: true } : { key: 'V', ctrlKey: true, shiftKey: true }), bubbles: true, cancelable: true }));
			await until(() => pty.inputs.join('').includes('pasted text'), 'Paste did not reach the shell');
		});
	});
	drawn('an exit code is explained, in decimal and hexadecimal, and the ended shell released', async () => {
		const pty = await fixture.start();
		pty.end({ code: 3221225786 });
		await fixture.waitForText('exited with code 3221225786 (0xC000013A). Press Enter to start a new shell.');
		assert.strictEqual(pty.shutdowns, 1);
	});
	drawn('after an exit, only Enter starts a new shell', async () => {
		const pty = await fixture.start();
		pty.end({ code: 3221225786 });
		await fixture.waitForText('Press Enter to start a new shell.');
		fixture.xterm().input('x', true);
		assert.strictEqual(fixture.ptys.length, 1, 'a key other than Enter started a new shell');
		fixture.enter();
		await until(() => fixture.ptys.length === 2 && fixture.ptys[1].started === 1, 'Enter did not start a new shell');
	});
	for (const [exit, message] of [[{ code: 0 }, 'The shell on Studio Mac ended.'], [{ reason: 'failed' }, 'The shell on Studio Mac stopped unexpectedly.'], [{ reason: 'lost' }, 'Lost the connection to Studio Mac.']] as const) {
		drawn(`a shell that ends with ${JSON.stringify(exit)} says "${message}", and Enter starts a new one`, async () => {
			const pty = await fixture.start();
			pty.end(exit);
			await fixture.waitForText(message);
			fixture.enter();
			await until(() => fixture.ptys.length === 2, 'Enter did not start a new shell');
		});
	}
	suite('a shell that cannot start', () => {
		setup(async () => {
			if (!webgl) {
				return;
			}
			const pty = await fixture.start();
			pty.end({ code: 0 });
			await until(() => pty.shutdowns === 1, 'The ended shell was not released');
		});
		for (const [code, message] of [
			['disconnected', 'Studio Mac isn\'t connected.'], ['notAllowed', 'Studio Mac doesn\'t allow Terminal for this computer.'], ['unsupported', 'Studio Mac can\'t open a terminal.'],
			['unavailable', 'Studio Mac can\'t open another shell right now.'], ['failed', 'The shell couldn\'t start on Studio Mac.'],
		] as const) {
			drawn(`the refusal ${code} is explained`, async () => {
				fixture.launchErrors.push({ code });
				fixture.enter();
				await fixture.waitForText(message);
			});
		}
		drawn('Enter after a refusal starts a new shell, and every replaced shell was ended', async () => {
			fixture.launchErrors.push({ code: 'failed' });
			fixture.enter();
			await fixture.waitForText('The shell couldn\'t start on Studio Mac.');
			const started = fixture.ptys.length;
			fixture.enter();
			await until(() => fixture.ptys.length === started + 1, 'Enter did not start a new shell after a refusal');
			assert.ok(fixture.ptys.slice(0, -1).every(pty => pty.shutdowns >= 1), 'A replaced shell was left running');
		});
	});
	drawn('the terminal\'s background is the palette\'s', async () => {
		await fixture.start();
		assert.strictEqual(fixture.xterm().options.theme?.background, getComputedStyle(fixture.view.element).backgroundColor);
	});
	drawn('a theme change reaches the terminal', async () => {
		await fixture.start();
		document.documentElement.style.setProperty('--terminal-ansi-red', '#123456');
		try {
			fixture.themeChanged.fire({ background: '#000000', foreground: '#ffffff', inactiveForeground: '#888888' });
			assert.strictEqual(fixture.xterm().options.theme?.red, 'rgb(18, 52, 86)');
		} finally {
			document.documentElement.style.removeProperty('--terminal-ansi-red');
		}
	});
	drawn('closing the tab ends its shell', async () => {
		const pty = await fixture.start();
		fixture.view.dispose();
		assert.strictEqual(pty.shutdowns, 1);
	});
});
