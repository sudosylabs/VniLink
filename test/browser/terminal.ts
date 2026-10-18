import { Emitter, Event } from '../../src/base/common/event.js';
import { DisposableStore } from '../../src/base/common/lifecycle.js';
import { isMacintosh } from '../../src/base/common/platform.js';
import { IClipboardService } from '../../src/platform/clipboard/common/clipboardService.js';
import { InstantiationService } from '../../src/platform/instantiation/common/instantiationService.js';
import { ServiceCollection } from '../../src/platform/instantiation/common/serviceCollection.js';
import type { IProcessDataEvent, ITerminalChildProcess, ITerminalLaunchError, TerminalExit } from '../../src/platform/terminal/common/terminal.js';
import { IThemeService, type ITitlebarTheme } from '../../src/platform/theme/common/themeService.js';
import { TerminalView } from '../../src/workbench/contrib/terminal/terminalView.js';
import '../../src/workbench/contrib/terminal/terminal.contribution.js';
import type { DeviceSummary } from '../../src/workbench/services/devices/deviceService.js';
import { ITabService } from '../../src/workbench/services/tabs/tabService.js';
import { TabService } from '../../src/workbench/services/tabs/tabServiceImpl.js';
import { ITerminalService } from '../../src/workbench/services/terminal/terminalService.js';

function assert(condition: boolean, message: string): void {
	if (!condition) {
		throw new Error(message);
	}
}

async function until(condition: () => boolean, message: string): Promise<void> {
	const deadline = Date.now() + 3000;
	while (!condition()) {
		assert(Date.now() < deadline, message);
		await new Promise(resolve => setTimeout(resolve, 20));
	}
}

/** A scripted shell: records what the view sends and lets the test answer. */
class FakePty implements ITerminalChildProcess {
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

/** The view's xterm, reached for assertions only. */
function xtermOf(view: TerminalView): XtermForTest {
	const xterm: unknown = Reflect.get(view, 'xterm');
	const raw: unknown = typeof xterm === 'object' && xterm !== null ? Reflect.get(xterm, 'raw') : undefined;
	assert(typeof raw === 'object' && raw !== null, 'The view has no terminal');
	return raw as XtermForTest;
}

function screenText(view: TerminalView): string {
	const buffer = xtermOf(view).buffer.active;
	let text = '';
	for (let index = 0; index < buffer.length; index++) {
		const line = buffer.getLine(index);
		// A wrapped row continues the previous line.
		const continued = buffer.getLine(index + 1)?.isWrapped === true;
		text += (line?.isWrapped ? '' : '\n') + (line?.translateToString(!continued) ?? '');
	}
	return text;
}

const device: DeviceSummary = { id: 'f'.repeat(64), name: 'Studio Mac', os: 'macos', presence: 'connected', hostname: 'studio.local', direct: 1, relays: 0 };

/**
 * The terminal tab under the window's real CSP with a scripted shell:
 * WebGL draws it, output is acknowledged once parsed, every way a shell
 * ends is explained, and Enter or Restart starts a new one.
 */
export async function verifyTerminal(): Promise<boolean> {
	const disposables = new DisposableStore();
	const violations: string[] = [];
	const onViolation = (event: SecurityPolicyViolationEvent): void => {
		violations.push(`${event.violatedDirective} ${event.blockedURI}`);
	};
	document.addEventListener('securitypolicyviolation', onViolation);
	const ptys: FakePty[] = [];
	const launchErrors: (ITerminalLaunchError | undefined)[] = [];
	let clipboard = '';
	const themeChanged = disposables.add(new Emitter<ITitlebarTheme>());
	const tabService = disposables.add(new TabService());
	const services = disposables.add(new InstantiationService(new ServiceCollection(
		[ITabService, tabService],
		[IThemeService, { _serviceBrand: undefined, onDidColorThemeChange: themeChanged.event, getColorTheme: () => ({ background: '#000000', foreground: '#ffffff', inactiveForeground: '#888888' }) }],
		[IClipboardService, {
			_serviceBrand: undefined,
			writeText: (text: string) => {
				clipboard = text;
				return Promise.resolve();
			},
			readText: () => Promise.resolve('pasted text'),
		}],
		[ITerminalService, {
			_serviceBrand: undefined,
			createProcess: (_id: string, cols: number, rows: number) => {
				const pty = new FakePty(cols, rows, launchErrors.shift());
				ptys.push(pty);
				return pty;
			},
		}],
	), true));
	const host = document.createElement('div');
	host.className = 'view-host';
	host.style.cssText = 'position:fixed;left:240px;top:80px;width:640px;height:360px;z-index:99;display:flex;';
	document.body.append(host);
	const view = services.createInstance(TerminalView, device);
	try {
		assert(view.id.startsWith(`terminal:${device.id}:`) && view.label === 'Studio Mac Terminal', `Tab identity is wrong: ${view.id} ${view.label}`);
		assert(view.element.getAttribute('role') === 'region' && view.element.getAttribute('aria-label') === 'Terminal on Studio Mac', 'The terminal region is not named');
		host.append(view.element);
		tabService.open(view);
		await until(() => ptys.length === 1 && ptys[0].started === 1, 'The shell did not start once the tab had a size');
		assert(view.element.querySelector('.xterm-screen canvas') !== null, 'WebGL did not draw the terminal');
		// The shell starts at the measured grid, not xterm's 80×24 default.
		assert(ptys[0].cols !== 80 && ptys[0].rows !== 24, `The shell started at ${String(ptys[0].cols)}×${String(ptys[0].rows)}`);
		await until(() => screenText(view).includes('Connecting to Studio Mac…'), 'The connecting message is missing');
		await ptys[0].output('hello \x1b[31mred\x1b[0m 你好\r\n');
		assert(ptys[0].acknowledged === 1 && screenText(view).includes('hello red 你好'), 'Output did not reach the terminal, or was not acknowledged');
		xtermOf(view).input('ls\r', true);
		assert(ptys[0].inputs.join('') === 'ls\r', `Typing did not reach the shell: ${JSON.stringify(ptys[0].inputs)}`);

		// Copy reaches the clipboard service; paste reads it back into the shell.
		xtermOf(view).selectAll();
		const textarea = view.element.querySelector('textarea');
		assert(textarea !== null, 'xterm has no input element');
		const copy = isMacintosh ? { key: 'c', metaKey: true } : { key: 'C', ctrlKey: true, shiftKey: true };
		const paste = isMacintosh ? { key: 'v', metaKey: true } : { key: 'V', ctrlKey: true, shiftKey: true };
		textarea?.dispatchEvent(new KeyboardEvent('keydown', { ...copy, bubbles: true, cancelable: true }));
		await until(() => clipboard.includes('hello red 你好'), 'Copy did not reach the clipboard');
		textarea?.dispatchEvent(new KeyboardEvent('keydown', { ...paste, bubbles: true, cancelable: true }));
		await until(() => ptys[0].inputs.join('').includes('pasted text'), 'Paste did not reach the shell');
		ptys[0].end({ code: 3221225786 });
		await until(() => screenText(view).includes('exited with code 3221225786 (0xC000013A). Press Enter to start a new shell.'), 'The exit is not explained');
		assert(ptys[0].shutdowns === 1, 'The ended shell was not released');
		xtermOf(view).input('x', true);
		assert(ptys.length === 1, 'A key other than Enter started a new shell');
		xtermOf(view).input('\r', true);
		await until(() => ptys.length === 2 && ptys[1].started === 1, 'Enter did not start a new shell');
		for (const [exit, message] of [[{ code: 0 }, 'The shell on Studio Mac ended.'], [{ reason: 'failed' }, 'The shell on Studio Mac stopped unexpectedly.'], [{ reason: 'lost' }, 'Lost the connection to Studio Mac.']] as const) {
			ptys.at(-1)?.end(exit);
			await until(() => screenText(view).includes(message), `Missing: ${message}`);
			view.restart();
		}
		for (const [code, message] of [
			['disconnected', 'Studio Mac isn\'t connected.'], ['notAllowed', 'Studio Mac doesn\'t allow Terminal for this computer.'], ['unsupported', 'Studio Mac can\'t open a terminal.'],
			['unavailable', 'Studio Mac can\'t open another shell right now.'], ['failed', 'The shell couldn\'t start on Studio Mac.'],
		] as const) {
			launchErrors.push({ code });
			view.restart();
			await until(() => screenText(view).includes(message), `Missing launch message: ${message}`);
		}
		const restarted = ptys.length;
		view.restart();
		await until(() => ptys.length === restarted + 1, 'Restart did not start a new shell');
		assert(ptys.slice(0, -1).every(pty => pty.shutdowns >= 1), 'A replaced shell was left running');

		// The theme comes from the palette tokens and follows a theme change.
		const background = getComputedStyle(view.element).backgroundColor;
		assert(xtermOf(view).options.theme?.background === background, `The terminal background is not the palette's: ${String(xtermOf(view).options.theme?.background)} vs ${background}`);
		document.documentElement.style.setProperty('--terminal-ansi-red', '#123456');
		themeChanged.fire({ background: '#000000', foreground: '#ffffff', inactiveForeground: '#888888' });
		assert(xtermOf(view).options.theme?.red === 'rgb(18, 52, 86)', 'A theme change did not reach the terminal');
		document.documentElement.style.removeProperty('--terminal-ansi-red');

		// xterm's DOM renderer and scrollbar write <style> elements the CSP refuses; nothing else may be blocked.
		const unexpected = violations.filter(violation => !violation.startsWith('style-src-elem inline'));
		assert(unexpected.length === 0, `Unexpected CSP violations: ${unexpected.join(', ')}`);
		const last = ptys.at(-1);
		view.dispose();
		assert(last?.shutdowns === 1, 'Closing the tab did not end its shell');
		return true;
	} finally {
		view.dispose();
		host.remove();
		disposables.dispose();
		document.removeEventListener('securitypolicyviolation', onViolation);
	}
}
