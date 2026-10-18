import { $, addDisposableListener, append, EventType, h } from '../../../base/browser/dom.js';
import { Disposable } from '../../../base/common/lifecycle.js';
import './media/access.css';

/** A switch row: title and description beside a `role="switch"` checkbox that starts disabled. */
export class AccessToggle extends Disposable {
	public readonly element: HTMLElement;
	public readonly input: HTMLInputElement;
	private readonly description: HTMLElement;

	constructor(title: string, description: string, change: (allowed: boolean) => void) {
		super();
		const row = h('label.access-toggle-row', [
			h('span', [h('span.access-toggle-title@title'), h('span.access-description@description')]),
		]);
		this.element = row.root;
		this.input = append(this.element, $<HTMLInputElement>('input.access-switch', { type: 'checkbox', role: 'switch', disabled: true }));
		this.description = row.description;
		row.title.textContent = title;
		this.description.textContent = description;
		this._register(addDisposableListener(this.input, EventType.CHANGE, () => {
			if (!this.input.disabled) {
				change(this.input.checked);
			}
		}));
	}

	public setValue(allowed: boolean): void {
		if (this.input.checked !== allowed) {
			this.input.checked = allowed;
		}
	}

	public setEnabled(enabled: boolean): void {
		if (this.input.disabled === enabled) {
			this.input.disabled = !enabled;
		}
	}

	public setDescription(description: string): void {
		if (this.description.textContent !== description) {
			this.description.textContent = description;
		}
	}

	public override dispose(): void {
		super.dispose();
		this.element.remove();
	}
}
