// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import { app } from 'electron';
import { VniLinkMain } from './code/electron-main/main.js';

new VniLinkMain().main().catch((error: unknown) => {
	// Startup failed before the application could log it.
	console.error(error);
	app.exit(1);
});
