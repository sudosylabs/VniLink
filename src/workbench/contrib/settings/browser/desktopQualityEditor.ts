import * as nls from '../../../../nls.js';
import { Disposable } from '../../../../base/common/lifecycle.js';
import { Emitter } from '../../../../base/common/event.js';
import { addDisposableListener } from '../../../../base/browser/domEvent.js';
import type { QualityLimits } from '../../../../platform/engine/common/engine.js';
import { isValidDesktopQuality, qualityPresets } from '../../../services/remoteDesktop/common/remoteDesktopService.js';

/** Edits a draft; only the page's explicit Apply action writes to the engine. */
export class DesktopQualityEditor extends Disposable {
	public readonly element = document.createElement('fieldset');
	private readonly changed = this._register(new Emitter<void>());
	public readonly onDidChange = this.changed.event;
	private readonly inputs = new Map<keyof QualityLimits, HTMLInputElement>();
	private readonly presets = new Map<string, HTMLButtonElement>();
	private readonly summary = document.createElement('p');

	constructor(legendTitle = nls.localize('desktop.quality', 'Stream quality')) {
		super();
		this.element.className = 'desktop-quality-editor';
		this.element.disabled = true;
		const legend = document.createElement('legend');
		legend.textContent = legendTitle;
		this.element.append(legend);
		const presetRow = document.createElement('div');
		presetRow.className = 'settings-control-row';
		const help = document.createElement('p');
		help.className = 'settings-help';
		help.textContent = nls.localize('desktop.presetHelp', 'Use a preset or choose your own limits.');
		const buttons = document.createElement('div');
		buttons.className = 'desktop-quality-presets';
		buttons.setAttribute('aria-label', nls.localize('desktop.presets', 'Quality presets'));
		for (const preset of qualityPresets) {
			const button = document.createElement('button');
			button.type = 'button';
			button.textContent = preset.label;
			button.dataset.preset = preset.id;
			button.setAttribute('aria-pressed', 'false');
			this.presets.set(preset.id, button);
			buttons.append(button);
			this._register(addDisposableListener(button, 'click', () => {
				if (!this.element.disabled) {
					this.setValue(preset.quality);
					this.changed.fire();
				}
			}));
		}
		presetRow.append(help, buttons);
		this.element.append(presetRow);
		this.addNumber('max_bitrate_kbps', nls.localize('desktop.bandwidth', 'Maximum bandwidth'), 'kbps', 50000, nls.localize('desktop.bandwidthHelp', 'Lower limits use less data and may reduce image detail.'));
		this.addNumber('max_frame_rate', nls.localize('desktop.frameRate', 'Maximum frame rate'), 'fps', 60, nls.localize('desktop.frameRateHelp', 'Higher limits make movement smoother when the connection allows it.'));
		const resolution = document.createElement('div');
		resolution.className = 'settings-control-row';
		const label = document.createElement('div');
		const title = document.createElement('span');
		title.className = 'desktop-setting-title';
		title.textContent = nls.localize('desktop.resolution', 'Maximum resolution');
		const description = document.createElement('p');
		description.className = 'settings-help';
		description.textContent = nls.localize('desktop.resolutionHelp', 'Limit frame dimensions without changing the host’s display resolution.');
		label.append(title, description);
		const dimensions = document.createElement('div');
		dimensions.className = 'desktop-dimensions';
		for (const key of ['max_width', 'max_height'] as const) {
			const name = key === 'max_width' ? nls.localize('desktop.width', 'Maximum width') : nls.localize('desktop.height', 'Maximum height');
			const input = this.number(key, 7680);
			input.setAttribute('aria-label', name);
			dimensions.append(input);
			if (key === 'max_width') {
				const cross = document.createElement('span');
				cross.textContent = '×';
				cross.setAttribute('aria-hidden', 'true');
				dimensions.append(cross);
			}
		}
		const unit = document.createElement('span');
		unit.textContent = 'px';
		dimensions.append(unit);
		resolution.append(label, dimensions);
		this.element.append(resolution);
		this.summary.className = 'settings-help desktop-quality-summary';
		this.element.append(this.summary);
	}

	private number(key: keyof QualityLimits, max: number): HTMLInputElement {
		const input = document.createElement('input');
		input.type = 'number';
		input.min = '1';
		input.max = String(max);
		input.step = '1';
		input.placeholder = '—';
		input.dataset.quality = key;
		this.inputs.set(key, input);
		this._register(addDisposableListener(input, 'input', () => {
			this.renderPreset();
			this.changed.fire();
		}));
		return input;
	}

	private addNumber(key: keyof QualityLimits, title: string, unit: string, max: number, help: string): void {
		const row = document.createElement('label');
		row.className = 'settings-control-row';
		const text = document.createElement('span');
		const name = document.createElement('span');
		name.className = 'desktop-setting-title';
		name.textContent = title;
		const description = document.createElement('span');
		description.className = 'settings-help';
		description.textContent = help;
		text.append(name, description);
		const control = document.createElement('span');
		control.className = 'desktop-number-control';
		const suffix = document.createElement('span');
		suffix.textContent = unit;
		control.append(this.number(key, max), suffix);
		row.append(text, control);
		this.element.append(row);
	}

	public value(): QualityLimits | undefined {
		const value = { max_bitrate_kbps: Number(this.inputs.get('max_bitrate_kbps')?.value), max_frame_rate: Number(this.inputs.get('max_frame_rate')?.value), max_width: Number(this.inputs.get('max_width')?.value), max_height: Number(this.inputs.get('max_height')?.value) };
		return isValidDesktopQuality(value) ? value : undefined;
	}

	public setValue(value: Readonly<QualityLimits> | undefined): void {
		for (const [key, input] of this.inputs) {
			input.value = value ? String(value[key]) : '';
		}
		this.renderPreset();
	}

	public setEnabled(enabled: boolean): void {
		this.element.disabled = !enabled;
	}

	private renderPreset(): void {
		const value = this.value();
		for (const preset of qualityPresets) {
			this.presets.get(preset.id)?.setAttribute('aria-pressed', String(!!value && Object.keys(value).every(key => value[key as keyof QualityLimits] === preset.quality[key as keyof QualityLimits])));
		}
		this.summary.textContent = value ? nls.localize('desktop.summary', 'Up to {0} Mb/s · {1} fps · {2} × {3} px', String(value.max_bitrate_kbps / 1000), String(value.max_frame_rate), String(value.max_width), String(value.max_height)) : '';
	}
}
