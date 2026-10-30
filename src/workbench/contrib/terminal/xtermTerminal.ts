/*---------------------------------------------------------------------------------------------
 *  Copyright (c) Microsoft Corporation. All rights reserved.
 *  Licensed under the MIT License. See LICENSE.vscode.txt in the project root for license information.
 *  Adapted from VS Code 1.141.0 src/vs/workbench/contrib/terminal/browser/xterm/xtermTerminal.ts.
 *--------------------------------------------------------------------------------------------*/
// ---------------------------------------------------------------------------------------------
// Modifications Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Modifications licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only AND MIT
// ---------------------------------------------------------------------------------------------

import { Terminal, type ITheme } from '@xterm/xterm';
import { Unicode11Addon } from '@xterm/addon-unicode11';
import { WebglAddon } from '@xterm/addon-webgl';
import { $, append } from '../../../base/browser/dom.js';
import { Emitter } from '../../../base/common/event.js';
import { Disposable, toDisposable } from '../../../base/common/lifecycle.js';
import { isMacintosh, isWindows } from '../../../base/common/platform.js';
import { IClipboardService } from '../../../platform/clipboard/common/clipboardService.js';
import { IThemeService } from '../../../platform/theme/common/themeService.js';

/**
 * Upstream `getXtermScaledDimensions` (contrib/terminal/browser/xterm/xtermTerminal.ts),
 * with VniLink's fixed letter spacing and line height.
 */
export function getXtermScaledDimensions(
	w: Window,
	charWidth: number,
	charHeight: number,
	width: number,
	height: number,
): { rows: number; cols: number } | null {
	if (!charWidth || !charHeight) {
		return null;
	}
	// xterm converts CSS pixels to device pixels through its canvas, so the
	// device pixel ratio keeps this exact when it changes.
	const scaledWidthAvailable = width * w.devicePixelRatio;
	const scaledCharWidth = charWidth * w.devicePixelRatio;
	const cols = Math.max(Math.floor(scaledWidthAvailable / scaledCharWidth), 1);
	const scaledHeightAvailable = height * w.devicePixelRatio;
	const scaledLineHeight = Math.ceil(charHeight * w.devicePixelRatio);
	const rows = Math.max(Math.floor(scaledHeightAvailable / scaledLineHeight), 1);
	return { rows, cols };
}

/** The renderer's measured cell, which xterm keeps on its private core (`_core._renderService.dimensions`), as VS Code reads it. */
function measuredCell(raw: Terminal): { readonly width: number; readonly height: number } | undefined {
	let value: unknown = raw;
	for (const key of ['_core', '_renderService', 'dimensions', 'css', 'cell']) {
		value = typeof value === 'object' && value !== null ? Reflect.get(value, key) : undefined;
	}
	if (typeof value !== 'object' || value === null) {
		return undefined;
	}
	const width: unknown = Reflect.get(value, 'width');
	const height: unknown = Reflect.get(value, 'height');
	return typeof width === 'number' && typeof height === 'number' ? { width, height } : undefined;
}

/** Palette tokens (colors.css) for each xterm theme slot. */
const themeTokens: readonly (readonly [keyof ITheme, string])[] = [
	['background', '--terminal-background'],
	['foreground', '--terminal-foreground'],
	['cursor', '--terminal-cursor'],
	['cursorAccent', '--terminal-background'],
	['selectionBackground', '--terminal-selection'],
	['selectionInactiveBackground', '--terminal-selection-inactive'],
	['black', '--terminal-ansi-black'],
	['red', '--terminal-ansi-red'],
	['green', '--terminal-ansi-green'],
	['yellow', '--terminal-ansi-yellow'],
	['blue', '--terminal-ansi-blue'],
	['magenta', '--terminal-ansi-magenta'],
	['cyan', '--terminal-ansi-cyan'],
	['white', '--terminal-ansi-white'],
	['brightBlack', '--terminal-ansi-bright-black'],
	['brightRed', '--terminal-ansi-bright-red'],
	['brightGreen', '--terminal-ansi-bright-green'],
	['brightYellow', '--terminal-ansi-bright-yellow'],
	['brightBlue', '--terminal-ansi-bright-blue'],
	['brightMagenta', '--terminal-ansi-bright-magenta'],
	['brightCyan', '--terminal-ansi-bright-cyan'],
	['brightWhite', '--terminal-ansi-bright-white'],
];

const fontFamily = isMacintosh ? 'ui-monospace, SFMono-Regular, Menlo, monospace'
	: isWindows ? '"Cascadia Mono", Consolas, monospace'
		: '"DejaVu Sans Mono", "Ubuntu Mono", monospace';

/**
 * One xterm.js terminal, a slim form of VS Code's `XtermTerminal`: WebGL
 * rendering (the window's CSP refuses the DOM renderer's style elements),
 * Unicode 11 widths, the workbench palette and the system clipboard.
 */
export class XtermTerminal extends Disposable {
	public readonly raw: Terminal;
	private readonly lostRenderer = this._register(new Emitter<void>());
	/** WebGL stopped and cannot draw; the terminal must be reopened. */
	public readonly onDidLoseRenderer = this.lostRenderer.event;
	private readonly probe = $('span.terminal-color-probe');
	private container: HTMLElement | undefined;

	constructor(
		@IThemeService themeService: IThemeService,
		@IClipboardService private readonly clipboardService: IClipboardService,
	) {
		super();
		this.raw = this._register(new Terminal({
			allowProposedApi: true,
			fontFamily,
			fontSize: 13,
			scrollback: 1000,
			macOptionIsMeta: false,
			rightClickSelectsWord: isMacintosh,
		}));
		const unicode = new Unicode11Addon();
		this.raw.loadAddon(unicode);
		this.raw.unicode.activeVersion = '11';
		this.raw.attachCustomKeyEventHandler(event => this.handleKey(event));
		this._register(themeService.onDidColorThemeChange(() => {
			this.applyTheme();
		}));
	}

	/** Opens xterm in `container` with the WebGL renderer. */
	public attachToElement(container: HTMLElement): void {
		this.container = container;
		this.probe.hidden = true;
		this.probe.style.forcedColorAdjust = 'none';
		append(container, this.probe);
		this._register(toDisposable(() => {
			this.probe.remove();
		}));
		this.applyTheme();
		this.raw.open(container);
		// xterm owns loaded addons and disposes them with the terminal.
		try {
			const webgl = new WebglAddon();
			this._register(webgl.onContextLoss(() => {
				webgl.dispose();
				this.lostRenderer.fire();
			}));
			this.raw.loadAddon(webgl);
		} catch {
			this.lostRenderer.fire();
		}
	}

	/** The grid that fits `width` by `height` CSS pixels, once a renderer measured its cell. */
	public getDimensions(width: number, height: number): { cols: number; rows: number } | undefined {
		const cell = measuredCell(this.raw);
		const window = this.container?.ownerDocument.defaultView;
		if (!cell || !window) {
			return undefined;
		}
		return getXtermScaledDimensions(window, cell.width, cell.height, width, height) ?? undefined;
	}

	public focus(): void {
		this.raw.focus();
	}

	private applyTheme(): void {
		if (!this.container) {
			return;
		}
		const theme: Record<string, string> = {};
		for (const [slot, token] of themeTokens) {
			this.probe.style.color = `var(${token})`;
			theme[slot] = getComputedStyle(this.probe).color;
		}
		this.raw.options.theme = theme;
	}

	/** Copy and paste keys reach the clipboard; everything else goes to the shell. */
	private handleKey(event: KeyboardEvent): boolean {
		if (event.type !== 'keydown') {
			return true;
		}
		const key = event.key.toLowerCase();
		const command = isMacintosh ? event.metaKey && !event.ctrlKey && !event.altKey : event.ctrlKey && !event.metaKey && !event.altKey;
		if (!command || (key !== 'c' && key !== 'v')) {
			return true;
		}
		// Ctrl+C is the shell's interrupt unless there is a selection to copy;
		// Linux pastes with Ctrl+Shift+V, leaving Ctrl+V to the shell.
		if (key === 'c') {
			if (!this.raw.hasSelection() && !isMacintosh && !event.shiftKey) {
				return true;
			}
			event.preventDefault();
			if (this.raw.hasSelection()) {
				void this.clipboardService.writeText(this.raw.getSelection()).catch(() => { /* The selection stays for another try. */ });
			}
			return false;
		}
		if (!isMacintosh && !isWindows && !event.shiftKey) {
			return true;
		}
		event.preventDefault();
		void this.clipboardService.readText().then(text => {
			if (text && !this._store.isDisposed) {
				this.raw.paste(text);
			}
		}, () => { /* Nothing to paste. */ });
		return false;
	}
}
