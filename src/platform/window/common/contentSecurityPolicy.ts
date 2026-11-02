// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

/**
 * The style elements a page may hold, by the hash of their text: xterm's
 * scrollbar rules with the colours `xtermTerminal.ts` fixes, and the empty
 * element xterm inserts before writing them. The terminal's tests check that
 * every style element xterm writes is one of these.
 */
export const allowedStyleHashes: readonly string[] = [
	'sha256-47DEQpj8HBSa+/TImW+5JCeuQeRkm5NMpJWZG3hSuFU=',
	'sha256-t/GMYduTuT6Jw+SWCIPMyUCpSk7aRNkLzU+CQ9KBizY=',
];

/**
 * Sent with every document the `vnilink://app/` protocol serves. A response header, unlike a
 * `<meta>` element, can carry `frame-ancestors`, so no other page can embed the workbench.
 * Images may be `data:` URLs: the other computer's pointer shape is drawn to a canvas and
 * shown as a CSS cursor, which takes an image URL there and then.
 */
export const contentSecurityPolicy = [
	"default-src 'self'",
	"script-src 'self'",
	`style-src 'self' ${allowedStyleHashes.map(hash => `'${hash}'`).join(' ')}`,
	"img-src 'self' data: blob:",
	"connect-src 'none'",
	"object-src 'none'",
	"base-uri 'none'",
	"frame-src 'none'",
	"frame-ancestors 'none'",
	"form-action 'none'",
].join('; ');
