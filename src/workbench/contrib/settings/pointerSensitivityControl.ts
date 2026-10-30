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
import { renderSettingRow, type ISettingControl } from './settingRenderers.js';

/** `input.pointerSensitivity` as a slider showing its percentage. */
export class PointerSensitivityControl extends Disposable implements ISettingControl {
	private readonly changed = this._register(new Emitter<unknown>());
	public readonly onDidChange = this.changed.event;
	public readonly continuous = true;
	private readonly slider = $<HTMLInputElement>('input.setting-slider', { type: 'range' });
	private readonly output = $('output');

	constructor(private readonly key: string, private readonly schema: ISettingSchema) {
		super();
	}

	public render(container: HTMLElement): void {
		const row = renderSettingRow(container, this.schema.title ?? this.key, this.schema.description, this.slider);
		this.slider.min = String(this.schema.minimum ?? 1);
		this.slider.max = String(this.schema.maximum ?? 400);
		this.slider.dataset.setting = this.key;
		row.control.append(this.slider, this.output);
		this._register(addDisposableListener(this.slider, EventType.INPUT, () => {
			this.renderOutput();
			this.changed.fire(Number(this.slider.value));
		}));
	}

	public setValue(value: unknown): void {
		this.slider.value = String(typeof value === 'number' ? value : this.schema.default);
		this.renderOutput();
	}

	public setEnabled(enabled: boolean): void {
		this.slider.disabled = !enabled;
	}

	private renderOutput(): void {
		this.output.textContent = `${this.slider.value}${this.schema.unit ?? ''}`;
	}
}
