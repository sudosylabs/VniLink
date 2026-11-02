// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import { DataChannelClient } from '../../../src/platform/engine/electron-browser/dataChannelClient.js';
import type { BrowserWindowDriver } from '../../../src/workbench/services/driver/driver.js';

/** The workbench's smoke-test driver; present only in isolated test launches. */
function windowDriver(): BrowserWindowDriver {
	const driver: unknown = Reflect.get(window, 'driver');
	if (typeof driver !== 'object' || driver === null) {
		throw new Error('Smoke-test driver unavailable');
	}
	return driver as BrowserWindowDriver;
}

/**
 * Exercises the real renderer, preload, main, utility and native handshake:
 * a channel to an unknown session fails promptly through the transferred
 * port, and the engine keeps answering afterwards.
 */
export async function verifyDataChannels(): Promise<void> {
	for (const kind of ['video', 'input', 'files', 'terminal'] as const) {
		let accepted = false;
		try {
			const channel = await DataChannelClient.open(windowDriver().engine, { kind, sessionId: '18446744073709551615', generation: '1', ...(kind === 'terminal' ? { rows: 24, columns: 80 } : {}) }, () => {
				throw new Error('Unknown session delivered data');
			}, () => {
				throw new Error('Startup failure escaped its promise');
			});
			accepted = true;
			channel.close();
		} catch { /* Unknown sessions must fail without losing the engine. */ }
		if (accepted) {
			throw new Error(`Unknown ${kind} data session was accepted`);
		}
	}
	const reply = await windowDriver().engine.execute({ type: 'snapshot' });
	if (reply.type !== 'snapshot') {
		throw new Error('Data-channel failure killed the engine');
	}
}
