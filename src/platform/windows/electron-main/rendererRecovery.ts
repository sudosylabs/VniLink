// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import type { RenderProcessGoneDetails } from 'electron';
import type { ILogService } from '../../log/common/log.js';

/** A renderer that dies again this soon after a reload is left as it is, so a crash on load cannot loop. */
const reloadAgainAfter = 30_000;

/** What recovery needs of a window, so it can be checked without Electron. */
export interface RecoverableWindow {
	readonly webContents: {
		on(event: 'render-process-gone', listener: (event: unknown, details: RenderProcessGoneDetails) => void): unknown;
		reload(): void;
	};
	on(event: 'unresponsive' | 'responsive', listener: () => void): unknown;
	isDestroyed(): boolean;
}

/**
 * Reloads a window whose renderer crashed or was killed, which would
 * otherwise stay blank, and logs a renderer that stops or starts answering.
 */
export function recoverRenderer(window: RecoverableWindow, logService: ILogService, now: () => number = Date.now): void {
	let reloadedAt = Number.NEGATIVE_INFINITY;
	window.webContents.on('render-process-gone', (_event, details) => {
		logService.error(`[window] renderer gone: ${details.reason} (exit code ${String(details.exitCode)})`);
		if (details.reason === 'clean-exit' || window.isDestroyed()) {
			return;
		}
		if (now() - reloadedAt < reloadAgainAfter) {
			logService.error('[window] the renderer died again right after a reload; it is not reloaded again');
			return;
		}
		reloadedAt = now();
		window.webContents.reload();
	});
	window.on('unresponsive', () => {
		logService.warn('[window] the renderer stopped responding');
	});
	window.on('responsive', () => {
		logService.info('[window] the renderer responds again');
	});
}
