// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import { open } from 'node:fs/promises';
import { basename, resolve } from 'node:path';
import { Emitter } from '../../../base/common/event.js';
import { Disposable } from '../../../base/common/lifecycle.js';
import { createDecorator } from '../../instantiation/common/instantiation.js';
import { ILogService } from '../../log/common/log.js';
import { isInvitationPath, MAX_INVITATION_BYTES, type OpenedInvitation } from '../common/invitations.js';

export const IInvitationsMainService = createDecorator<InvitationsMainService>('invitationsMainService');

/** Reads a file of at most `limit` bytes, or `undefined` when it is not a regular file or is larger. */
export type InvitationReader = (path: string, limit: number) => Promise<Uint8Array | undefined>;

export async function readInvitation(path: string, limit: number): Promise<Uint8Array | undefined> {
	const file = await open(path, 'r');
	try {
		const metadata = await file.stat();
		if (!metadata.isFile() || metadata.size === 0 || metadata.size > limit) {
			return undefined;
		}
		const bytes = new Uint8Array(metadata.size);
		const { bytesRead } = await file.read(bytes, 0, bytes.byteLength, 0);
		return bytesRead === bytes.byteLength ? bytes : undefined;
	} finally {
		await file.close();
	}
}

/**
 * Holds the invitations the operating system opened with VniLink until a
 * window takes them: from the command line of the first launch and of later
 * launches (Windows and Linux) and from macOS's `open-file`. Only files named
 * as invitations and small enough to be one are read; their contents are
 * still untrusted, and the engine checks them when the window opens them.
 */
export class InvitationsMainService extends Disposable {
	private readonly pending: OpenedInvitation[] = [];
	private readonly changed = this._register(new Emitter<void>());
	/** Fires when invitations are waiting for a window to take them. */
	public readonly onDidOpenInvitations = this.changed.event;

	constructor(
		private readonly reader: InvitationReader,
		@ILogService private readonly logService: ILogService,
	) {
		super();
	}

	/** Reads the invitations among `argv`, resolving relative paths against `workingDirectory`. */
	public async openFromCommandLine(argv: readonly string[], workingDirectory: string): Promise<boolean> {
		return this.openPaths(argv.filter(isInvitationPath).map(path => resolve(workingDirectory, path)));
	}

	/** Reads the invitations at absolute `paths`; whether any was accepted. */
	public async openPaths(paths: readonly string[]): Promise<boolean> {
		let accepted = false;
		for (const path of paths) {
			if (!isInvitationPath(path)) {
				continue;
			}
			try {
				const bytes = await this.reader(path, MAX_INVITATION_BYTES);
				if (bytes) {
					this.pending.push({ name: basename(path), bytes: Array.from(bytes) });
					accepted = true;
				} else {
					this.logService.warn(`Not an invitation VniLink can open: ${basename(path)}`);
				}
			} catch (error) {
				this.logService.warn(`The invitation ${basename(path)} could not be read`, error);
			}
		}
		if (accepted) {
			this.changed.fire();
		}
		return accepted;
	}

	/** The waiting invitations, oldest first; each is handed out once. */
	public take(): readonly OpenedInvitation[] {
		return this.pending.splice(0);
	}
}
