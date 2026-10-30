// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import * as nls from '../../../nls.js';
import { $, addDisposableListener, append, EventType, h } from '../../../base/browser/dom.js';
import { renderIcon } from '../../../base/browser/ui/icon/icon.js';
import { Emitter } from '../../../base/common/event.js';
import { Icons } from '../../../base/common/icons.js';
import { Disposable } from '../../../base/common/lifecycle.js';
import type { RelayInfo, RelayInput } from '../../../platform/engine/common/engine.js';
import type { RelayValidationIssue } from '../../../platform/engine/common/relayValidation.js';

let nextId = 0;

/** A labelled field with its help text, as the relay rows lay them out. */
function createField(label: string, control: HTMLElement): { readonly root: HTMLElement; readonly description: HTMLElement } {
	const field = h('.setting', [h('label@caption', [h('span.setting-label@label')]), h('p.setting-description@description')]);
	field.label.textContent = label;
	field.caption.append(control);
	return field;
}

/** Owns one relay draft. Saved credentials are never read back into this view. */
export class RelayEditor extends Disposable {
	public readonly element = $<HTMLFieldSetElement>('fieldset.relay-editor');
	private readonly url = $<HTMLInputElement>('input', { type: 'text', placeholder: 'https://relay.example.com', spellcheck: 'false' });
	private readonly authenticated = $<HTMLInputElement>('input.relay-auth-switch', { type: 'checkbox', role: 'switch' });
	private readonly token = $<HTMLInputElement>('input', { type: 'password', autocomplete: 'new-password', spellcheck: 'false' });
	private readonly tokenHelp: HTMLElement;
	private readonly error = $('p.setting-error', { id: `relay-error-${String(++nextId)}` });
	private readonly changed = this._register(new Emitter<void>());
	/** Fires when the user finishes an edit: leaves a field or flips the switch. */
	public readonly onDidChange = this.changed.event;
	private readonly removed = this._register(new Emitter<void>());
	public readonly onDidRemove = this.removed.event;

	constructor(relay: RelayInfo | undefined) {
		super();
		append(this.element, $('legend.connection-visually-hidden')).textContent = nls.localize('connection.relay', 'Custom relay');
		this.url.value = relay?.url ?? '';
		this.authenticated.checked = relay?.authenticated ?? false;
		this.error.hidden = true;
		this.url.setAttribute('aria-describedby', this.error.id);
		this.token.setAttribute('aria-describedby', this.error.id);
		const removeLabel = nls.localize('connection.removeRelay', 'Remove relay');
		const remove = $<HTMLButtonElement>('button.relay-remove', { type: 'button', 'aria-label': removeLabel, title: removeLabel }, renderIcon(Icons.close));
		append(this.element, $('.relay-address-row', undefined, createField(nls.localize('connection.url', 'Relay URL'), this.url).root, remove));
		const authRow = append(this.element, $('label.relay-auth-row'));
		append(authRow, $('span')).textContent = nls.localize('connection.authentication', 'Authentication required');
		authRow.append(this.authenticated);
		const field = createField(nls.localize('connection.token', 'Authentication token'), this.token);
		const tokenField = append(this.element, field.root);
		this.tokenHelp = field.description;
		this.setSavedToken(relay?.authenticated ?? false);
		this.element.append(this.error);
		const updateToken = (): void => {
			this.token.disabled = !this.authenticated.checked;
			tokenField.hidden = !this.authenticated.checked;
			if (!this.authenticated.checked) {
				this.token.value = '';
			}
		};
		updateToken();
		this._register(addDisposableListener(remove, EventType.CLICK, () => {
			this.removed.fire();
		}));
		this._register(addDisposableListener(this.authenticated, EventType.CHANGE, () => {
			updateToken();
			this.changed.fire();
		}));
		for (const input of [this.url, this.token]) {
			this._register(addDisposableListener(input, EventType.CHANGE, () => {
				this.changed.fire();
			}));
		}
	}

	public get value(): RelayInput {
		return { url: this.url.value.trim(), authenticated: this.authenticated.checked, replacementToken: this.authenticated.checked ? this.token.value : '' };
	}

	/** After a save the token is stored: clear it, and offer to keep it. */
	public setSavedToken(saved: boolean): void {
		this.token.value = '';
		this.tokenHelp.textContent = saved
			? nls.localize('connection.keepToken', 'Leave blank to keep the saved token for this exact URL.')
			: nls.localize('connection.newToken', 'Enter the token provided by your relay operator.');
	}

	public showIssues(issues: readonly RelayValidationIssue[]): void {
		this.error.textContent = issues.map(issue => issue.message).join(' ');
		this.error.hidden = !issues.length;
		this.url.setAttribute('aria-invalid', String(issues.some(issue => issue.field === 'url')));
		this.token.setAttribute('aria-invalid', String(issues.some(issue => issue.field === 'token')));
	}

	public setEnabled(enabled: boolean): void {
		this.element.disabled = !enabled;
	}

	public override dispose(): void {
		this.token.value = '';
		super.dispose();
		this.element.remove();
	}
}
