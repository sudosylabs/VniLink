import * as nls from '../../../nls.js';
import { $, addDisposableListener, append, EventType } from '../../../base/browser/dom.js';
import { Emitter } from '../../../base/common/event.js';
import { Disposable } from '../../../base/common/lifecycle.js';
import type { ISettingSchema } from '../../../platform/configuration/common/configurationSchema.js';
import { isQualityLimits, type QualityLimits } from '../../../platform/engine/common/engine.js';
import { qualityPresets } from '../../services/remoteDesktop/remoteDesktopService.js';
import type { ISettingControl } from './settingRenderers.js';
import './media/remoteDesktop.css';

/** `desktop.quality`: presets that set all four limits, or the limits one by one. */
export class DesktopQualityControl extends Disposable implements ISettingControl {
	private readonly changed = this._register(new Emitter<unknown>());
	public readonly onDidChange = this.changed.event;
	public readonly continuous = true;
	private readonly element = $<HTMLFieldSetElement>('fieldset.desktop-quality-editor', { disabled: true });
	private readonly inputs = new Map<keyof QualityLimits, HTMLInputElement>();
	private readonly presets = new Map<string, HTMLButtonElement>();
	private readonly summary = $('p.settings-help.desktop-quality-summary');

	constructor(_key: string, private readonly schema: ISettingSchema) {
		super();
	}

	public render(container: HTMLElement): void {
		append(this.element, $('legend')).textContent = this.schema.title ?? nls.localize('desktop.quality', 'Stream quality');
		if (this.schema.description) {
			append(this.element, $('p.settings-help')).textContent = this.schema.description;
		}
		const presetRow = append(this.element, $('.settings-control-row'));
		append(presetRow, $('p.settings-help')).textContent = nls.localize('desktop.presetHelp', 'Use a preset or choose your own limits.');
		const buttons = append(presetRow, $('.desktop-quality-presets', { 'aria-label': nls.localize('desktop.presets', 'Quality presets') }));
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
		this.addNumber('max_bitrate_kbps', nls.localize('desktop.bandwidth', 'Maximum bandwidth'), 'kbps', 50000, nls.localize('desktop.bandwidthHelp', 'Lower limits use less data and may reduce image detail.'));
		this.addNumber('max_frame_rate', nls.localize('desktop.frameRate', 'Maximum frame rate'), 'fps', 60, nls.localize('desktop.frameRateHelp', 'Higher limits make movement smoother when the connection allows it.'));
		const resolution = append(this.element, $('.settings-control-row'));
		const label = append(resolution, $('div'));
		append(label, $('span.desktop-setting-title')).textContent = nls.localize('desktop.resolution', 'Maximum resolution');
		append(label, $('p.settings-help')).textContent = nls.localize('desktop.resolutionHelp', 'Limit frame dimensions without changing the host’s display resolution.');
		const dimensions = append(resolution, $('.desktop-dimensions'));
		dimensions.append(this.number('max_width', 7680, nls.localize('desktop.width', 'Maximum width')));
		append(dimensions, $('span', { 'aria-hidden': 'true' })).textContent = '×';
		dimensions.append(this.number('max_height', 7680, nls.localize('desktop.height', 'Maximum height')));
		append(dimensions, $('span')).textContent = 'px';
		this.element.append(this.summary);
		container.append(this.element);
	}

	private number(key: keyof QualityLimits, max: number, label?: string): HTMLInputElement {
		const input = $<HTMLInputElement>('input', { type: 'number', min: '1', max: String(max), step: '1', placeholder: '—', 'data-quality': key, 'aria-label': label });
		this.inputs.set(key, input);
		this._register(addDisposableListener(input, EventType.INPUT, () => {
			this.renderPreset();
			this.changed.fire(this.value());
		}));
		return input;
	}

	private addNumber(key: keyof QualityLimits, title: string, unit: string, max: number, help: string): void {
		const row = append(this.element, $('label.settings-control-row'));
		const text = append(row, $('span'));
		append(text, $('span.desktop-setting-title')).textContent = title;
		append(text, $('span.settings-help')).textContent = help;
		const control = append(row, $('span.desktop-number-control'));
		control.append(this.number(key, max));
		append(control, $('span')).textContent = unit;
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
		for (const preset of qualityPresets) {
			const pressed = !!value && [...this.inputs.keys()].every(key => value[key] === preset.quality[key]);
			this.presets.get(preset.id)?.setAttribute('aria-pressed', String(pressed));
		}
		this.summary.textContent = value ? nls.localize('desktop.summary', 'Up to {0} Mb/s · {1} fps · {2} × {3} px', String(value.max_bitrate_kbps / 1000), String(value.max_frame_rate), String(value.max_width), String(value.max_height)) : '';
	}
}
