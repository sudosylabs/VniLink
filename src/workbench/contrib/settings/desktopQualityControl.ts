// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import * as nls from '../../../nls.js';
import { $, addDisposableListener, append, EventType } from '../../../base/browser/dom.js';
import { Emitter } from '../../../base/common/event.js';
import { Disposable } from '../../../base/common/lifecycle.js';
import type { ISettingSchema } from '../../../platform/configuration/common/configurationSchema.js';
import { isQualityLimits, type QualityLimits } from '../../../platform/engine/common/engine.js';
import { bandwidthLimits, defaultMaximumBandwidth } from '../../services/remoteDesktop/remoteDesktopService.js';
import { renderSettingRow, type ISettingControl } from './settingRenderers.js';

type Bandwidth = 'automatic' | 'dataSaver' | 'maximum';

/** `desktop.quality`: how much bandwidth a stream may use, and the frame rate and size it may reach. */
export class DesktopQualityControl extends Disposable implements ISettingControl {
	private readonly changed = this._register(new Emitter<unknown>());
	public readonly onDidChange = this.changed.event;
	public readonly continuous = true;
	private readonly element = $<HTMLFieldSetElement>('fieldset.desktop-quality-editor', { disabled: true });
	private readonly inputs = new Map<keyof QualityLimits, HTMLInputElement>();
	private readonly choices = new Map<Bandwidth, HTMLButtonElement>();
	private help: HTMLElement | undefined;
	/** Kept while a maximum is typed, so a value equal to another choice's does not take the field away. */
	private maximum = false;

	constructor(_key: string, private readonly schema: ISettingSchema) {
		super();
	}

	public render(container: HTMLElement): void {
		this.element.setAttribute('aria-label', this.schema.title ?? nls.localize('desktop.quality', 'Stream limits'));
		const labels: Readonly<Record<Bandwidth, string>> = {
			automatic: nls.localize('desktop.bandwidth.automatic', 'Automatic'),
			dataSaver: nls.localize('desktop.bandwidth.dataSaver', 'Data saver'),
			maximum: nls.localize('desktop.bandwidth.maximum', 'Maximum'),
		};
		const row = renderSettingRow(this.element, nls.localize('desktop.bandwidth', 'Bandwidth'), undefined);
		this.help = row.help;
		const group = append(row.control, $('.settings-segmented', { role: 'group', 'aria-label': nls.localize('desktop.bandwidthChoices', 'Bandwidth'), 'aria-describedby': row.help.id }));
		for (const choice of ['automatic', 'dataSaver', 'maximum'] as const) {
			const button = append(group, $<HTMLButtonElement>('button', { type: 'button', 'data-bandwidth': choice, 'aria-pressed': 'false' }));
			button.textContent = labels[choice];
			this.choices.set(choice, button);
			this._register(addDisposableListener(button, EventType.CLICK, () => {
				this.choose(choice);
			}));
		}
		const bandwidth = this.number('max_bitrate_kbps', 50000, nls.localize('desktop.bandwidthLimit', 'Maximum bandwidth'));
		const limit = renderSettingRow(this.element, nls.localize('desktop.bandwidthLimit', 'Maximum bandwidth'), nls.localize('desktop.bandwidthLimitHelp', 'Used when Bandwidth is Maximum.'), bandwidth);
		limit.control.append(bandwidth);
		append(limit.control, $('span.setting-unit')).textContent = 'kbps';
		const frameRate = this.number('max_frame_rate', 60, nls.localize('desktop.frameRate', 'Frame rate limit'));
		const rate = renderSettingRow(this.element, nls.localize('desktop.frameRate', 'Frame rate limit'), nls.localize('desktop.frameRateHelp', 'Pictures a second at most. Lowered when a computer or the connection can’t keep up.'), frameRate);
		rate.control.append(frameRate);
		append(rate.control, $('span.setting-unit')).textContent = 'fps';
		const width = this.number('max_width', 7680, nls.localize('desktop.width', 'Width'));
		const resolution = renderSettingRow(this.element, nls.localize('desktop.resolution', 'Size limit'), nls.localize('desktop.resolutionHelp', 'Doesn’t change the other computer’s display.'), width);
		resolution.control.append(width);
		append(resolution.control, $('span', { 'aria-hidden': 'true' })).textContent = '×';
		resolution.control.append(this.number('max_height', 7680, nls.localize('desktop.height', 'Height')));
		append(resolution.control, $('span.setting-unit')).textContent = 'px';
		container.append(this.element);
	}

	private number(key: keyof QualityLimits, max: number, label: string): HTMLInputElement {
		const input = $<HTMLInputElement>('input.setting-number', { type: 'number', min: '1', max: String(max), step: '1', placeholder: '—', 'data-quality': key, 'aria-label': label });
		this.inputs.set(key, input);
		this._register(addDisposableListener(input, EventType.INPUT, () => {
			this.renderBandwidth();
			this.changed.fire(this.value());
		}));
		return input;
	}

	private choose(choice: Bandwidth): void {
		if (this.element.disabled) {
			return;
		}
		const input = this.inputs.get('max_bitrate_kbps');
		if (!input) {
			return;
		}
		this.maximum = choice === 'maximum';
		input.value = String(choice === 'automatic' ? bandwidthLimits.automatic : choice === 'dataSaver' ? bandwidthLimits.dataSaver : defaultMaximumBandwidth);
		this.renderBandwidth();
		this.changed.fire(this.value());
		if (this.maximum) {
			input.focus();
		}
	}

	/** The edited limits, or `undefined` while any limit is out of range. */
	public value(): QualityLimits | undefined {
		const value = Object.fromEntries([...this.inputs].map(([key, input]) => [key, Number(input.value)]));
		return isQualityLimits(value) ? value : undefined;
	}

	public setValue(value: unknown): void {
		const quality = isQualityLimits(value) ? value : undefined;
		for (const [key, input] of this.inputs) {
			input.value = quality ? String(quality[key]) : '';
		}
		this.maximum = !!quality && quality.max_bitrate_kbps !== bandwidthLimits.automatic && quality.max_bitrate_kbps !== bandwidthLimits.dataSaver;
		this.renderBandwidth();
	}

	public setEnabled(enabled: boolean): void {
		this.element.disabled = !enabled;
		this.renderBandwidth();
	}

	private renderBandwidth(): void {
		const kbps = Number(this.inputs.get('max_bitrate_kbps')?.value);
		const chosen: Bandwidth = this.maximum ? 'maximum' : kbps === bandwidthLimits.dataSaver ? 'dataSaver' : kbps === bandwidthLimits.automatic ? 'automatic' : 'maximum';
		for (const [choice, button] of this.choices) {
			button.setAttribute('aria-pressed', String(choice === chosen));
		}
		const input = this.inputs.get('max_bitrate_kbps');
		if (input) {
			input.disabled = this.element.disabled || chosen !== 'maximum';
		}
		if (!this.help) {
			return;
		}
		this.help.textContent = chosen === 'automatic'
			? nls.localize('desktop.bandwidth.automaticHelp', 'Follows the picture and the connection.')
			: chosen === 'dataSaver'
				? nls.localize('desktop.bandwidth.dataSaverHelp', 'Keeps data use low on a metered connection, with less detail.')
				: nls.localize('desktop.bandwidth.maximumHelp', 'Follows the picture and the connection, up to the limit below.');
	}
}
