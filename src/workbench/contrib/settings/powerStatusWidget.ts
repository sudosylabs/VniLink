// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import * as nls from '../../../nls.js';
import { $ } from '../../../base/browser/dom.js';
import { Disposable } from '../../../base/common/lifecycle.js';
import { INativeRuntimeService } from '../../services/nativeRuntime/nativeRuntimeService.js';
import type { ISettingsWidget } from './settingsLayout.js';

/** Reports when the operating system refuses to keep this computer awake. */
export class PowerStatusWidget extends Disposable implements ISettingsWidget {
	private readonly element = $('p.settings-help');

	constructor(@INativeRuntimeService private readonly runtime: INativeRuntimeService) {
		super();
	}

	public render(container: HTMLElement): void {
		container.append(this.element);
	}

	public async refresh(): Promise<void> {
		let message = '';
		try {
			const failures = (await this.runtime.powerStatus()).filter(status => status.phase === 'Failed' || status.phase === 'Unavailable');
			if (failures.length) {
				message = nls.localize('power.failed', 'Sleep prevention is unavailable: {0}', failures.map(status => status.error ?? status.phase).join(', '));
			}
		} catch {
			// The power settings report an unavailable engine themselves.
		}
		if (!this.isDisposed()) {
			this.element.textContent = message;
		}
	}

	private isDisposed(): boolean {
		return this._store.isDisposed;
	}
}
