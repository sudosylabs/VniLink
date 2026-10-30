// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import { context } from './base/parts/sandbox/electron-browser/globals.js';
import { isNativeWindowConfiguration } from './platform/window/common/windowConfiguration.js';
import { main } from './workbench/workbench.desktop.main.js';

void (async () => {
	const configuration: unknown = await context.resolveConfiguration();
	if (!isNativeWindowConfiguration(configuration)) {
		throw new Error('Invalid window configuration');
	}
	await main(configuration);
})().catch((error: unknown) => {
	console.error(error);
	const status = document.querySelector('[role="status"]');
	if (status) {
		status.textContent = 'The desktop could not start. Close the window and try again.';
	}
});
