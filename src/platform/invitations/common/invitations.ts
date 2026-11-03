// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import { product } from '../../product/common/product.js';

/** Main-process IPC channel handing windows the invitations the operating system opened with VniLink. */
export const INVITATIONS_CHANNEL_NAME = 'invitations';

/** Invitations larger than this are not VniLink invitations. */
export const MAX_INVITATION_BYTES = 65_536;

/**
 * An invitation file the operating system asked VniLink to open: its name and
 * contents, read in the main process. The bytes are a plain array because the
 * channel carries objects as JSON.
 */
export interface OpenedInvitation {
	readonly name: string;
	readonly bytes: readonly number[];
}

/** Whether a command-line argument or path names an invitation by its extension, whatever its case. */
export function isInvitationPath(path: string): boolean {
	return !path.startsWith('-') && path.toLowerCase().endsWith(`.${product.invitationExtension}`);
}

function isOpenedInvitation(value: unknown): value is OpenedInvitation {
	if (typeof value !== 'object' || value === null) {
		return false;
	}
	const { name, bytes } = value as Record<string, unknown>;
	return typeof name === 'string' && Array.isArray(bytes) && bytes.length > 0 && bytes.length <= MAX_INVITATION_BYTES
		&& bytes.every(byte => Number.isInteger(byte) && (byte as number) >= 0 && (byte as number) <= 255);
}

export function isOpenedInvitations(value: unknown): value is readonly OpenedInvitation[] {
	return Array.isArray(value) && value.every(isOpenedInvitation);
}
