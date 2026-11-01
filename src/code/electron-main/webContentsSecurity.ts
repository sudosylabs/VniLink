// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import type { WebContents } from 'electron';

/**
 * Sent with every document the `vnilink://app/` protocol serves. A response header, unlike a
 * `<meta>` element, can carry `frame-ancestors`, so no other page can embed the workbench.
 */
export const contentSecurityPolicy = [
	"default-src 'self'",
	"script-src 'self'",
	"style-src 'self'",
	"img-src 'self' blob:",
	"connect-src 'none'",
	"object-src 'none'",
	"base-uri 'none'",
	"frame-src 'none'",
	"frame-ancestors 'none'",
	"form-action 'none'",
].join('; ');

/**
 * Every page VniLink shows is a bundled one, so no web contents may open a window, attach a
 * webview or leave the page it loaded. Registered for all web contents, including ones created
 * later, so a window added elsewhere starts out guarded.
 */
export function secureWebContents(contents: WebContents): void {
	contents.setWindowOpenHandler(() => ({ action: 'deny' }));
	contents.on('will-navigate', event => {
		event.preventDefault();
	});
	contents.on('will-redirect', event => {
		event.preventDefault();
	});
	contents.on('will-attach-webview', event => {
		event.preventDefault();
	});
}
