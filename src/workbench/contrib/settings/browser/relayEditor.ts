import * as nls from '../../../../nls.js';
import { Disposable } from '../../../../base/common/lifecycle.js';
import { addDisposableListener } from '../../../../base/browser/domEvent.js';
import type { RelayInput, RelayInfo } from '../../../../platform/engine/common/engine.js';
import { renderIcon } from '../../../../base/browser/ui/icon/icon.js';
import { Icons } from '../../../../base/common/icons.js';
import { createSetting } from './settingsControls.js';
import type { RelayValidationIssue } from '../../../services/connection/common/relayValidation.js';

let nextId = 0;

/** Owns one relay draft. Saved credentials are never read back into this view. */
export class RelayEditor extends Disposable {
	public readonly element = document.createElement('fieldset');
	private readonly url = document.createElement('input');
	private readonly authenticated = document.createElement('input');
	private readonly token = document.createElement('input');
	private readonly error = document.createElement('p');

	constructor(relay: RelayInfo | undefined, remove: () => void) {
		super();
		this.element.className = 'relay-editor';
		const legend = document.createElement('legend');
		legend.className = 'connection-visually-hidden';
		legend.textContent = nls.localize('connection.relay', 'Custom relay');
		this.url.type = 'text';
		this.url.value = relay?.url ?? '';
		this.url.placeholder = 'https://relay.example.com';
		this.url.spellcheck = false;
		this.authenticated.type = 'checkbox';
		this.authenticated.checked = relay?.authenticated ?? false;
		this.token.type = 'password';
		this.token.autocomplete = 'new-password';
		this.token.spellcheck = false;
		this.token.disabled = !this.authenticated.checked;
		const removeButton = document.createElement('button');
		removeButton.type = 'button';
		removeButton.className = 'relay-remove';
		const removeLabel = nls.localize('connection.removeRelay', 'Remove relay');
		removeButton.setAttribute('aria-label', removeLabel);
		removeButton.title = removeLabel;
		removeButton.append(renderIcon(Icons.close));
		this.error.className = 'setting-error';
		this.error.id = `relay-error-${String(++nextId)}`;
		this.error.hidden = true;
		this.url.setAttribute('aria-describedby', this.error.id);
		this.token.setAttribute('aria-describedby', this.error.id);
		const address = document.createElement('div');
		address.className = 'relay-address-row';
		address.append(createSetting(nls.localize('connection.url', 'Relay URL'), '', this.url), removeButton);
		const authRow = document.createElement('label');
		authRow.className = 'relay-auth-row';
		const authLabel = document.createElement('span');
		authLabel.textContent = nls.localize('connection.authentication', 'Authentication required');
		this.authenticated.className = 'relay-auth-switch';
		this.authenticated.setAttribute('role', 'switch');
		authRow.append(authLabel, this.authenticated);
		const tokenField = createSetting(nls.localize('connection.token', 'Authentication token'), relay?.authenticated
			? nls.localize('connection.keepToken', 'Leave blank to keep the saved token for this exact URL.')
			: nls.localize('connection.newToken', 'Enter the token provided by your relay operator.'), this.token);
		tokenField.hidden = !this.authenticated.checked;
		this.element.append(legend, address, authRow, tokenField, this.error);
		this._register(addDisposableListener(removeButton, 'click', remove));
		this._register(addDisposableListener(this.authenticated, 'change', () => {
			this.token.disabled = !this.authenticated.checked;
			tokenField.hidden = !this.authenticated.checked;
			if (!this.authenticated.checked) {
				this.token.value = '';
			}
		}));
	}

	public get value(): RelayInput {
		return { url: this.url.value.trim(), authenticated: this.authenticated.checked, replacementToken: this.authenticated.checked ? this.token.value : '' };
	}

	public showIssues(issues: readonly RelayValidationIssue[]): void {
		this.error.textContent = issues.map(issue => issue.message).join(' ');
		this.error.hidden = !issues.length;
		this.url.setAttribute('aria-invalid', String(issues.some(issue => issue.field === 'url')));
		this.token.setAttribute('aria-invalid', String(issues.some(issue => issue.field === 'token')));
	}

	public clearToken(): void {
		this.token.value = '';
	}

	public override dispose(): void {
		this.clearToken();
		super.dispose();
		this.element.remove();
	}
}
