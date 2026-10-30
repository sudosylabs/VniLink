// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import type { BrowserWindowDriver } from '../../src/workbench/services/driver/driver.js';

/** The workbench's smoke-test driver; present only in isolated test launches. */
export function windowDriver(): BrowserWindowDriver {
	const driver: unknown = Reflect.get(window, 'driver');
	if (typeof driver !== 'object' || driver === null) {
		throw new Error('Smoke-test driver unavailable');
	}
	return driver as BrowserWindowDriver;
}
