// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import type { IClipboardService } from '../../../platform/clipboard/common/clipboardService.js';

/** An {@link IClipboardService} that keeps what is written; reading returns the last of it. */
export class TestClipboardService implements IClipboardService {
	declare public readonly _serviceBrand: undefined;
	public readonly written: string[] = [];

	public writeText(text: string): Promise<void> {
		this.written.push(text);
		return Promise.resolve();
	}

	public readText(): Promise<string> {
		return Promise.resolve(this.written.at(-1) ?? '');
	}
}
