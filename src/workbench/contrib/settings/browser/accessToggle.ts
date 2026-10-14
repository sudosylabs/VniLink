import { Disposable } from '../../../../base/common/lifecycle.js';
import { addDisposableListener } from '../../../../base/browser/domEvent.js';

export class AccessToggle extends Disposable {
	public readonly element = document.createElement('label');
	public readonly input = document.createElement('input');
	private readonly description = document.createElement('span');

	constructor(title: string, description: string, change: (allowed: boolean) => void) {
		super();
		this.element.className = 'access-toggle-row';
		const text = document.createElement('span');
		const label = document.createElement('span');
		label.className = 'access-toggle-title';
		label.textContent = title;
		this.description.className = 'access-description';
		this.description.textContent = description;
		text.append(label, this.description);
		this.input.type = 'checkbox';
		this.input.setAttribute('role', 'switch');
		this.input.className = 'access-switch';
		this.input.disabled = true;
		this.element.append(text, this.input);
		this._register(addDisposableListener(this.input, 'change', () => {
			if (!this.input.disabled) {
				change(this.input.checked);
			}
		}));
	}

	public setValue(allowed: boolean): void {
		this.input.checked = allowed;
	}

	public setEnabled(enabled: boolean): void {
		this.input.disabled = !enabled;
	}

	public setDescription(description: string): void {
		this.description.textContent = description;
	}

	public override dispose(): void {
		super.dispose();
		this.element.remove();
	}
}
