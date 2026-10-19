import * as nls from '../../../nls.js';
import { $, addDisposableListener, append, EventType } from '../../../base/browser/dom.js';
import { Emitter } from '../../../base/common/event.js';
import { Disposable } from '../../../base/common/lifecycle.js';
import type { ISettingSchema } from '../../../platform/configuration/common/configurationSchema.js';
import { isQualityLimits, type QualityLimits } from '../../../platform/engine/common/engine.js';
import { qualityPresets } from '../../services/remoteDesktop/remoteDesktopService.js';
import { renderSettingRow, type ISettingControl } from './settingRenderers.js';

/** `desktop.quality`: a preset that sets all four limits, or the limits one by one. */
export class DesktopQualityControl extends Disposable implements ISettingControl {
	private readonly changed = this._register(new Emitter<unknown>());
	public readonly onDidChange = this.changed.event;
	public readonly continuous = true;
	private readonly element = $<HTMLFieldSetElement>('fieldset.desktop-quality-editor', { disabled: true });
	private readonly inputs = new Map<keyof QualityLimits, HTMLInputElement>();
	private readonly presets = new Map<string, HTMLButtonElement>();
	/** Shown pressed while the limits match no preset; choosing it changes nothing. */
	private readonly custom = $<HTMLButtonElement>('button', { type: 'button', 'data-preset': 'custom', 'aria-pressed': 'false' });

	constructor(_key: string, private readonly schema: ISettingSchema) {
		super();
	}

	public render(container: HTMLElement): void {
		this.element.setAttribute('aria-label', this.schema.title ?? nls.localize('desktop.quality', 'Stream quality'));
		const presetRow = renderSettingRow(this.element, nls.localize('desktop.preset', 'Preset'), undefined);
		const buttons = append(presetRow.control, $('.settings-segmented', { role: 'group', 'aria-label': nls.localize('desktop.presets', 'Quality presets') }));
		for (const preset of qualityPresets) {
			const button = append(buttons, $<HTMLButtonElement>('button', { type: 'button', 'data-preset': preset.id, 'aria-pressed': 'false' }));
			button.textContent = preset.label;
			this.presets.set(preset.id, button);
			this._register(addDisposableListener(button, EventType.CLICK, () => {
				if (!this.element.disabled) {
					this.setValue(preset.quality);
					this.changed.fire(this.value());
				}
			}));
		}
		this.custom.textContent = nls.localize('desktop.custom', 'Custom');
		buttons.append(this.custom);
		this._register(addDisposableListener(this.custom, EventType.CLICK, () => {
			this.inputs.get('max_bitrate_kbps')?.focus();
		}));
		this.addNumber('max_bitrate_kbps', nls.localize('desktop.bandwidth', 'Bandwidth'), 'kbps', 50000, nls.localize('desktop.bandwidthHelp', 'Less uses less data, with less detail.'));
		this.addNumber('max_frame_rate', nls.localize('desktop.frameRate', 'Frame rate'), 'fps', 60);
		const width = this.number('max_width', 7680, nls.localize('desktop.width', 'Width'));
		const resolution = renderSettingRow(this.element, nls.localize('desktop.resolution', 'Resolution'), nls.localize('desktop.resolutionHelp', 'Doesn’t change the other computer’s display.'), width);
		resolution.control.append(width);
		append(resolution.control, $('span', { 'aria-hidden': 'true' })).textContent = '×';
		resolution.control.append(this.number('max_height', 7680, nls.localize('desktop.height', 'Height')));
		append(resolution.control, $('span.setting-unit')).textContent = 'px';
		container.append(this.element);
	}

	private number(key: keyof QualityLimits, max: number, label?: string): HTMLInputElement {
		const input = $<HTMLInputElement>('input.setting-number', { type: 'number', min: '1', max: String(max), step: '1', placeholder: '—', 'data-quality': key, 'aria-label': label });
		this.inputs.set(key, input);
		this._register(addDisposableListener(input, EventType.INPUT, () => {
			this.renderPreset();
			this.changed.fire(this.value());
		}));
		return input;
	}

	private addNumber(key: keyof QualityLimits, title: string, unit: string, max: number, help?: string): void {
		const input = this.number(key, max);
		const row = renderSettingRow(this.element, title, help, input);
		row.control.append(input);
		append(row.control, $('span.setting-unit')).textContent = unit;
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
		this.renderPreset();
	}

	public setEnabled(enabled: boolean): void {
		this.element.disabled = !enabled;
	}

	private renderPreset(): void {
		const value = this.value();
		let matched = false;
		for (const preset of qualityPresets) {
			const pressed = !!value && [...this.inputs.keys()].every(key => value[key] === preset.quality[key]);
			matched ||= pressed;
			this.presets.get(preset.id)?.setAttribute('aria-pressed', String(pressed));
		}
		this.custom.setAttribute('aria-pressed', String(!matched));
	}
}
