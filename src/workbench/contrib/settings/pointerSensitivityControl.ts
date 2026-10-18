import { $, addDisposableListener, append, EventType } from '../../../base/browser/dom.js';
import { Emitter } from '../../../base/common/event.js';
import { Disposable } from '../../../base/common/lifecycle.js';
import type { ISettingSchema } from '../../../platform/configuration/common/configurationSchema.js';
import type { ISettingControl } from './settingRenderers.js';
import './media/input.css';

/** `input.pointerSensitivity` as a slider showing its percentage. */
export class PointerSensitivityControl extends Disposable implements ISettingControl {
	private readonly changed = this._register(new Emitter<unknown>());
	public readonly onDidChange = this.changed.event;
	public readonly continuous = true;
	private readonly slider = $<HTMLInputElement>('input', { type: 'range' });
	private readonly output = $('output');

	constructor(private readonly key: string, private readonly schema: ISettingSchema) {
		super();
	}

	public render(container: HTMLElement): void {
		const row = append(container, $('label.settings-control-row'));
		append(row, $('span')).textContent = this.schema.title ?? this.key;
		const control = append(row, $('span.input-sensitivity'));
		this.slider.min = String(this.schema.minimum ?? 1);
		this.slider.max = String(this.schema.maximum ?? 400);
		this.slider.dataset.setting = this.key;
		control.append(this.slider, this.output);
		if (this.schema.description) {
			append(container, $('p.settings-help')).textContent = this.schema.description;
		}
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
