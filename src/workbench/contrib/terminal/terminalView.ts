// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import * as nls from '../../../nls.js';
import { $, append } from '../../../base/browser/dom.js';
import { Icons } from '../../../base/common/icons.js';
import { Disposable, DisposableStore, MutableDisposable, toDisposable } from '../../../base/common/lifecycle.js';
import { IInstantiationService } from '../../../platform/instantiation/common/instantiation.js';
import { MAX_TERMINAL_COLUMNS, MAX_TERMINAL_ROWS, type ITerminalChildProcess, type TerminalExit, type TerminalLaunchFailure } from '../../../platform/terminal/common/terminal.js';
import { formatMessageForTerminal } from '../../../platform/terminal/common/terminalStrings.js';
import type { DeviceSummary } from '../../services/devices/deviceService.js';
import { ITabService, type WorkbenchTab } from '../../services/tabs/tabService.js';
import { ITerminalService } from '../../services/terminal/terminalService.js';
import { TerminalResizeDebouncer } from './terminalResizeDebouncer.js';
import { XtermTerminal } from './xtermTerminal.js';

/** Grid used before the renderer has measured a cell. */
const defaultSize = { cols: 80, rows: 24 };

let nextTerminal = 1;

function exitMessage(name: string, exit: TerminalExit): string {
	if ('reason' in exit) {
		return exit.reason === 'lost'
			? nls.localize('terminal.lost', 'Lost the connection to {0}.', name)
			: nls.localize('terminal.failed', 'The shell on {0} stopped unexpectedly.', name);
	}
	return exit.code === 0
		? nls.localize('terminal.ended', 'The shell on {0} ended.', name)
		: nls.localize('terminal.exitCode', 'The shell on {0} exited with code {1}.', name, exitCodeLabel(exit.code));
}

/** Windows status codes read as hexadecimal, as Windows writes them (3221225786 is 0xC000013A). */
function exitCodeLabel(code: number): string {
	return code > 0xffff ? `${String(code)} (0x${code.toString(16).toUpperCase().padStart(8, '0')})` : String(code);
}

function launchMessage(name: string, failure: TerminalLaunchFailure): string {
	switch (failure) {
		case 'disconnected': return nls.localize('terminal.disconnected', '{0} isn\'t connected.', name);
		case 'notAllowed': return nls.localize('terminal.notAllowed', '{0} doesn\'t allow Terminal for this computer.', name);
		case 'unsupported': return nls.localize('terminal.unsupported', '{0} can\'t open a terminal.', name);
		case 'unavailable': return nls.localize('terminal.unavailable', '{0} can\'t open another shell right now.', name);
		case 'failed': return nls.localize('terminal.launchFailed', 'The shell couldn\'t start on {0}.', name);
	}
}

/**
 * A tab with a shell on one paired computer. When the shell ends, its output
 * stays readable and Enter starts a new one in the same tab.
 */
export class TerminalView extends Disposable implements WorkbenchTab {
	public static readonly PREFIX = 'terminal:';
	public readonly id: string;
	public readonly label: string;
	public readonly icon = Icons.terminal;
	public readonly element: HTMLElement;
	private readonly host: HTMLElement;
	private readonly xterm: XtermTerminal;
	private readonly process = this._register(new MutableDisposable<ITerminalChildProcess>());
	private readonly processListeners = this._register(new DisposableStore());
	private readonly resizeDebouncer: TerminalResizeDebouncer;
	private attached = false;
	private ended = false;
	private launch = 0;

	constructor(
		private readonly device: DeviceSummary,
		@IInstantiationService instantiationService: IInstantiationService,
		@ITerminalService private readonly terminalService: ITerminalService,
		@ITabService private readonly tabService: ITabService,
	) {
		super();
		this.id = `${TerminalView.PREFIX}${device.id}:${String(nextTerminal++)}`;
		this.label = nls.localize('terminal.label', '{0} Terminal', device.name);
		this.element = $('.terminal-view');
		this.element.setAttribute('role', 'region');
		this.element.setAttribute('aria-label', nls.localize('terminal.region', 'Terminal on {0}', device.name));
		this.host = append(this.element, $('.terminal-host'));
		// The shell's process is disposed after xterm, which may still hold its output.
		this.xterm = this._register(instantiationService.createInstance(XtermTerminal));
		this.resizeDebouncer = this._register(new TerminalResizeDebouncer(
			() => this.isVisible(),
			() => this.xterm.raw,
			(cols, rows) => {
				this.xterm.raw.resize(cols, rows);
				this.process.value?.resize(cols, rows);
			},
			cols => {
				this.xterm.raw.resize(cols, this.xterm.raw.rows);
				this.process.value?.resize(cols, this.xterm.raw.rows);
			},
			rows => {
				this.xterm.raw.resize(this.xterm.raw.cols, rows);
				this.process.value?.resize(this.xterm.raw.cols, rows);
			},
		));
		this.registerListeners();
	}

	private registerListeners(): void {
		this._register(this.xterm.raw.onData(data => {
			if (this.ended) {
				if (data === '\r') {
					this.start();
				}
				return;
			}
			this.process.value?.input(data);
		}));
		this._register(this.xterm.raw.onBinary(data => {
			if (!this.ended) {
				this.process.value?.processBinary(data);
			}
		}));
		this._register(this.xterm.onDidLoseRenderer(() => {
			this.showRendererLost();
		}));
		// The first size the tab is given attaches xterm, so the shell starts at the size it is shown.
		const observer = new ResizeObserver(() => {
			this.layout();
		});
		observer.observe(this.host);
		this._register(toDisposable(() => {
			observer.disconnect();
		}));
		this._register(this.tabService.onDidChange(() => {
			if (this.tabService.activeTab === this && this.attached) {
				this.resizeDebouncer.flush();
				this.xterm.focus();
			}
		}));
	}

	private isVisible(): boolean {
		return this.host.clientWidth > 0 && this.host.clientHeight > 0;
	}

	private layout(): void {
		if (!this.isVisible()) {
			return;
		}
		const first = !this.attached;
		if (first) {
			this.attached = true;
			this.xterm.attachToElement(this.host);
			// Without a renderer nothing could show the shell, so none starts.
			if (this.host.hidden) {
				return;
			}
		}
		const size = this.xterm.getDimensions(this.host.clientWidth, this.host.clientHeight);
		if (size) {
			this.resizeDebouncer.resize(Math.min(size.cols, MAX_TERMINAL_COLUMNS), Math.min(size.rows, MAX_TERMINAL_ROWS), first);
		}
		if (first) {
			this.start();
			this.xterm.focus();
		}
	}

	private start(): void {
		this.processListeners.clear();
		this.process.clear();
		const launch = ++this.launch;
		this.ended = false;
		const size = this.xterm.raw.element ? { cols: this.xterm.raw.cols, rows: this.xterm.raw.rows } : defaultSize;
		const pty = this.terminalService.createProcess(this.device.id, size.cols, size.rows);
		this.process.value = pty;
		this.processListeners.add(pty.onProcessData(event => {
			this.xterm.raw.write(event.data, () => {
				event.acknowledge();
			});
		}));
		this.processListeners.add(pty.onProcessExit(exit => {
			this.stop(exitMessage(this.device.name, exit));
		}));
		this.write(nls.localize('terminal.connecting', 'Connecting to {0}…', this.device.name), launch === 1);
		void pty.start().then(error => {
			if (error && launch === this.launch && !this._store.isDisposed) {
				this.stop(launchMessage(this.device.name, error.code));
			}
		});
	}

	/** Leaves the output readable and offers a new shell. */
	private stop(message: string): void {
		this.ended = true;
		this.processListeners.clear();
		this.process.clear();
		this.write(`${message} ${nls.localize('terminal.pressEnter', 'Press Enter to start a new shell.')}`, false);
	}

	private write(message: string, first: boolean): void {
		this.xterm.raw.write(formatMessageForTerminal(message, { excludeLeadingNewLine: first }));
	}

	private showRendererLost(): void {
		this.process.clear();
		this.processListeners.clear();
		this.ended = true;
		this.host.hidden = true;
		const notice = append(this.element, $('p.terminal-notice'));
		notice.setAttribute('role', 'alert');
		notice.textContent = nls.localize('terminal.rendererLost', 'The terminal can\'t draw on this computer\'s graphics. Close this tab and open the terminal again.');
	}
}
