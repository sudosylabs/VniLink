import * as nls from '../../../nls.js';
import { $, addDisposableListener, append, EventType } from '../../../base/browser/dom.js';
import { Emitter } from '../../../base/common/event.js';
import { Disposable } from '../../../base/common/lifecycle.js';

export type RelayMode = 'public' | 'custom';

let nextPickerId = 0;

export class RelayModePicker extends Disposable {
	public readonly element = $<HTMLFieldSetElement>('fieldset.relay-mode-picker');
	private readonly changed = this._register(new Emitter<RelayMode>());
	public readonly onDidChange = this.changed.event;
	private readonly inputs = new Map<RelayMode, HTMLInputElement>();

	constructor() {
		super();
		append(this.element, $('legend.connection-visually-hidden')).textContent = nls.localize('connection.mode', 'Relay mode');
		const name = `relay-mode-${String(++nextPickerId)}`;
		const choices: readonly (readonly [RelayMode, string])[] = [
			['public', nls.localize('connection.public', 'Public relays')],
			['custom', nls.localize('connection.custom', 'Custom relays')],
		];
		for (const [mode, title] of choices) {
			const label = append(this.element, $('label.relay-mode-choice'));
			const input = append(label, $<HTMLInputElement>('input', { type: 'radio', name, value: mode }));
			input.checked = mode === 'public';
			append(append(label, $('span')), $('span.relay-mode-title')).textContent = title;
			this.inputs.set(mode, input);
			this._register(addDisposableListener(input, EventType.CHANGE, () => {
				if (input.checked && !input.matches(':disabled')) {
					this.changed.fire(mode);
				}
			}));
		}
	}

	public get value(): RelayMode {
		return this.inputs.get('custom')?.checked ? 'custom' : 'public';
	}

	public setValue(mode: RelayMode): void {
		for (const [key, input] of this.inputs) {
			input.checked = key === mode;
		}
	}

	public override dispose(): void {
		super.dispose();
		this.inputs.clear();
		this.element.remove();
	}
}
