// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import { $, addDisposableListener, EventType } from '../../../base/browser/dom.js';
import { Emitter } from '../../../base/common/event.js';
import { Disposable } from '../../../base/common/lifecycle.js';
import type { ISettingSchema } from '../../../platform/configuration/common/configurationSchema.js';
import { MAX_COMPUTER_NAME_BYTES } from '../../../platform/engine/common/engine.js';
import { IDeviceService } from '../../services/devices/deviceService.js';
import { renderSettingRow, type ISettingControl } from './settingRenderers.js';

/**
 * `application.computerName`: a text field whose placeholder is the
 * operating system's name, which an empty field leaves in use. The name is
 * saved after a pause in typing, without the spaces around it.
 */
export class ComputerNameControl extends Disposable implements ISettingControl {
	private readonly changed = this._register(new Emitter<unknown>());
	public readonly onDidChange = this.changed.event;
	public readonly continuous = true;
	private readonly input = $<HTMLInputElement>('input.setting-text', { type: 'text', spellcheck: 'false', autocomplete: 'off' });

	constructor(private readonly key: string, private readonly schema: ISettingSchema, @IDeviceService private readonly devices: IDeviceService) {
		super();
	}

	public render(container: HTMLElement): void {
		const row = renderSettingRow(container, this.schema.title ?? this.key, this.schema.description, this.input);
		this.input.dataset.setting = this.key;
		this.input.maxLength = MAX_COMPUTER_NAME_BYTES;
		row.control.append(this.input);
		this._register(addDisposableListener(this.input, EventType.INPUT, () => {
			this.changed.fire(this.input.value.trim());
		}));
		void this.refresh();
	}

	public setValue(value: unknown): void {
		// Typing keeps its spaces until the field is left; a saved value replaces it only when it differs.
		if (typeof value === 'string' && value !== this.input.value.trim()) {
			this.input.value = value;
		}
	}

	public setEnabled(enabled: boolean): void {
		this.input.disabled = !enabled;
	}

	public async refresh(): Promise<void> {
		const system = await this.devices.systemComputerName().catch(() => null);
		if (!this._store.isDisposed) {
			this.input.placeholder = system ?? '';
		}
	}
}
