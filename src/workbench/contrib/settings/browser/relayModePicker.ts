import * as nls from '../../../../nls.js';
import { Disposable } from '../../../../base/common/lifecycle.js';
import { Emitter } from '../../../../base/common/event.js';
import { addDisposableListener } from '../../../../base/browser/domEvent.js';

export type RelayMode = 'public' | 'custom';

let nextPickerId = 0;

export class RelayModePicker extends Disposable {
	public readonly element = document.createElement('fieldset');
	private readonly changed = this._register(new Emitter<RelayMode>());
	public readonly onDidChange = this.changed.event;
	private readonly inputs = new Map<RelayMode, HTMLInputElement>();

	constructor() {
		super();
		this.element.className = 'relay-mode-picker';
		const legend = document.createElement('legend');
		legend.className = 'connection-visually-hidden';
		legend.textContent = nls.localize('connection.mode', 'Relay mode');
		this.element.append(legend);
		const name = `relay-mode-${String(++nextPickerId)}`;
		const choices: readonly (readonly [RelayMode, string])[] = [
			['public', nls.localize('connection.public', 'Public relays')],
			['custom', nls.localize('connection.custom', 'Custom relays')],
		];
		for (const [mode, title] of choices) {
			const label = document.createElement('label');
			label.className = 'relay-mode-choice';
			const input = document.createElement('input');
			input.type = 'radio';
			input.name = name;
			input.value = mode;
			input.checked = mode === 'public';
			const text = document.createElement('span');
			const heading = document.createElement('span');
			heading.className = 'relay-mode-title';
			heading.textContent = title;
			text.append(heading);
			label.append(input, text);
			this.element.append(label);
			this.inputs.set(mode, input);
			this._register(addDisposableListener(input, 'change', () => {
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
