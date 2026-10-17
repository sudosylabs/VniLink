import * as nls from '../../../nls.js';
import { addDisposableListener, EventType, h, triggerDownload, triggerUpload } from '../../../base/browser/dom.js';
import { StandardKeyboardEvent } from '../../../base/browser/keyboardEvent.js';
import { Button } from '../../../base/browser/ui/button/button.js';
import { InputBox } from '../../../base/browser/ui/inputbox/inputBox.js';
import { RunOnceScheduler } from '../../../base/common/async.js';
import { KeyCode } from '../../../base/common/keyCodes.js';
import { Disposable } from '../../../base/common/lifecycle.js';
import { defaultButtonStyles, defaultInputBoxStyles } from '../../../platform/theme/browser/defaultStyles.js';
import { IDeviceService } from '../../services/devices/deviceService.js';

/** Invitations larger than this are not VniLink invitations. */
const maxInvitationSize = 65_536;

/** How often an open invitation checks for an incoming pairing request. */
const requestPollInterval = 500;

/** Digits in a pairing confirmation code. */
const codePattern = /^[0-9]{3}$/;

/**
 * Presents invitations and pairing confirmation. Pairing and authorization
 * policy live in the engine; this view only carries files and digits.
 */
export class DevicePairing extends Disposable {
	public readonly element: HTMLElement;
	private readonly heading: HTMLElement;
	private readonly description: HTMLElement;
	private readonly code: HTMLElement;
	private readonly form: HTMLElement;
	private readonly message: HTMLElement;
	private readonly digits: InputBox;
	private readonly submit: Button;
	private readonly reject: Button;
	private readonly dismiss: Button;
	private preview: Uint8Array | undefined;
	private incomingId: string | undefined;
	private expiresAt = 0;
	private awaitingEndpoint: string | undefined;
	private busy = false;
	private revision = 0;
	// Incoming requests have no engine event, so an open invitation polls for them.
	private readonly polling = this._register(new RunOnceScheduler(() => void this.poll(), requestPollInterval));

	constructor(@IDeviceService private readonly devices: IDeviceService) {
		super();
		const section = h('section.device-pairing', [
			h('h2@heading'),
			h('p.device-help@description'),
			h('output.device-pairing-code@code', { 'aria-label': nls.localize('devices.code', 'Confirmation code') }),
			h('div.device-pairing-form@form', [h('label@label')]),
			h('p.device-feedback@message', { role: 'status' }),
		]);
		this.element = section.root;
		this.element.hidden = true;
		this.heading = section.heading;
		this.description = section.description;
		this.code = section.code;
		this.form = section.form;
		this.form.hidden = true;
		this.message = section.message;
		section.label.append(nls.localize('devices.enterCode', 'Code shown on the other computer'));
		this.digits = this._register(new InputBox(section.label, undefined, { inputBoxStyles: defaultInputBoxStyles }));
		this.digits.inputElement.inputMode = 'numeric';
		this.digits.inputElement.autocomplete = 'off';
		this.digits.inputElement.maxLength = 3;
		this.submit = this.createButton(this.form, nls.localize('devices.confirm', 'Confirm'), true);
		this.reject = this.createButton(this.form, nls.localize('devices.reject', 'Reject'));
		this.dismiss = this.createButton(this.element, nls.localize('devices.dismiss', 'Close'));
		this.registerListeners();
		this.setBusy(false);
	}

	private createButton(container: HTMLElement, label: string, primary = false): Button {
		const button = this._register(new Button(container, { ...defaultButtonStyles, secondary: !primary, title: false }));
		button.label = label;
		button.element.classList.add('device-button', ...(primary ? ['primary'] : []));
		return button;
	}

	private registerListeners(): void {
		this._register(this.devices.onDidChangeDevices(known => {
			if (this.awaitingEndpoint && known.some(device => device.id === this.awaitingEndpoint && device.presence === 'connected')) {
				this.awaitingEndpoint = undefined;
				this.code.textContent = '';
				this.message.textContent = nls.localize('devices.paired', 'Computer paired. Your sharing settings still control what it can access.');
			}
		}));
		this._register(addDisposableListener(this.digits.inputElement, EventType.KEY_DOWN, event => {
			if (new StandardKeyboardEvent(event).equals(KeyCode.Enter)) {
				event.preventDefault();
				this.confirmCode();
			}
		}));
		this._register(this.submit.onDidClick(() => {
			this.confirmCode();
		}));
		this._register(this.reject.onDidClick(() => {
			this.run(async () => {
				await this.devices.rejectPairing();
				this.incomingId = undefined;
				this.form.hidden = true;
				this.message.textContent = nls.localize('devices.rejected', 'Pairing request rejected.');
			});
		}));
		this._register(this.dismiss.onDidClick(() => {
			this.revision++;
			this.preview = undefined;
			this.element.hidden = true;
		}));
	}

	/** Opens an invitation from another computer; without `file`, asks the user to choose one. */
	public importInvitation(file?: File): void {
		if (this.busy || this.incomingId) {
			return;
		}
		this.run(async () => {
			const chosen = file ?? (await triggerUpload())?.[0];
			if (chosen) {
				await this.open(chosen);
			}
		});
	}

	/** Creates an invitation for this computer and downloads it. */
	public shareInvitation(): void {
		if (!this.busy && !this.incomingId) {
			this.run(() => this.share());
		}
	}

	private begin(title: string, description: string): number {
		this.element.hidden = false;
		this.heading.textContent = title;
		this.description.textContent = description;
		this.code.textContent = '';
		this.message.textContent = '';
		this.digits.value = '';
		this.form.hidden = true;
		this.preview = undefined;
		this.awaitingEndpoint = undefined;
		return ++this.revision;
	}

	private async open(file: File): Promise<void> {
		const revision = this.begin(nls.localize('devices.importTitle', 'Pair another computer'), nls.localize('devices.importHelp', 'Ask the person who created this invitation for the three digits shown on their screen.'));
		if (file.size > maxInvitationSize || !file.size) {
			throw new Error(nls.localize('devices.fileSize', 'Choose a valid .vnl invitation smaller than 64 KB.'));
		}
		const bytes = new Uint8Array(await file.arrayBuffer());
		if (!this.current(revision)) {
			return;
		}
		this.preview = bytes;
		this.incomingId = undefined;
		this.form.hidden = false;
		this.reject.element.hidden = true;
		this.digits.focus();
	}

	private async share(): Promise<void> {
		const revision = this.begin(nls.localize('devices.shareTitle', 'Invite another computer'), nls.localize('devices.shareHelp', 'Send the .vnl file and share these digits with the other person. Keep VniLink open. The invitation expires in 15 minutes.'));
		const file = await this.devices.exportPairingFile();
		if (!this.current(revision)) {
			return;
		}
		triggerDownload(new Uint8Array(file.bytes), 'VniLink.vnl');
		this.code.textContent = file.check;
		this.expiresAt = file.expiresAt;
		this.polling.schedule();
	}

	private confirmCode(): void {
		if (!codePattern.test(this.digits.value)) {
			this.message.textContent = nls.localize('devices.codeFormat', 'Enter the three digits shown on the other computer.');
			this.digits.focus();
			return;
		}
		this.run(() => this.confirm());
	}

	private async confirm(): Promise<void> {
		const revision = this.revision;
		if (this.preview) {
			const endpoint = await this.devices.pair(this.preview, this.digits.value);
			const code = await this.devices.pairingCheck();
			if (!this.current(revision)) {
				return;
			}
			this.preview = undefined;
			this.form.hidden = true;
			this.code.textContent = code;
			this.description.textContent = nls.localize('devices.reverseCheck', 'Share these digits with the other person so they can approve this computer.');
			this.awaitingEndpoint = endpoint;
			await this.devices.connect(endpoint);
		} else if (this.incomingId) {
			const request = await this.devices.incomingPairing();
			if (request?.endpoint_id !== this.incomingId) {
				throw new Error(nls.localize('devices.requestChanged', 'The pairing request expired or changed. Wait for the other computer to try again.'));
			}
			await this.devices.approvePairing(this.digits.value);
			if (this.current(revision)) {
				this.incomingId = undefined;
				this.expiresAt = 0;
				this.form.hidden = true;
				this.code.textContent = '';
				this.message.textContent = nls.localize('devices.paired', 'Computer paired. Your sharing settings still control what it can access.');
			}
		}
	}

	private async poll(): Promise<void> {
		if (!this.expiresAt || this.isDisposed()) {
			return;
		}
		if (Date.now() >= this.expiresAt) {
			this.expiresAt = 0;
			this.code.textContent = '';
			this.form.hidden = true;
			this.incomingId = undefined;
			this.message.textContent = nls.localize('devices.expired', 'The invitation expired. Share a new invitation to pair.');
			return;
		}
		try {
			const request = await this.devices.incomingPairing();
			if (this.isDisposed() || this.busy || this.preview) {
				return;
			}
			if (request && request.endpoint_id !== this.incomingId) {
				this.begin(nls.localize('devices.approveTitle', 'Approve this computer'), nls.localize('devices.approveHelp', 'Ask the other person for the three digits on their screen before pairing.'));
				this.incomingId = request.endpoint_id;
				this.form.hidden = false;
				this.reject.element.hidden = false;
				this.setBusy(false);
			} else if (!request && this.incomingId) {
				this.incomingId = undefined;
				this.form.hidden = true;
				this.setBusy(false);
				this.message.textContent = nls.localize('devices.requestExpired', 'The pairing request ended. The other computer can try again.');
			}
		} catch {
			if (!this.isDisposed()) {
				this.message.textContent = nls.localize('devices.approvalUnavailable', 'Unable to check incoming pairing requests. Retrying…');
			}
		} finally {
			if (!this.isDisposed() && this.expiresAt) {
				this.polling.schedule();
			}
		}
	}

	/** Re-read after every await: the view can close while a request is in flight. */
	private isDisposed(): boolean {
		return this._store.isDisposed;
	}

	private current(revision: number): boolean {
		return !this.isDisposed() && this.revision === revision;
	}

	private run(action: () => Promise<void>): void {
		if (this.busy || this.isDisposed()) {
			return;
		}
		this.setBusy(true);
		action().catch((error: unknown) => {
			if (!this.isDisposed()) {
				this.element.hidden = false;
				this.message.textContent = error instanceof Error ? error.message : nls.localize('devices.pairingFailed', 'Pairing failed. Try again.');
			}
		}).finally(() => {
			if (!this.isDisposed()) {
				this.setBusy(false);
			}
		});
	}

	private setBusy(value: boolean): void {
		this.busy = value;
		this.element.setAttribute('aria-busy', String(value));
		const enabled = !value && this.devices.available;
		this.submit.enabled = enabled;
		this.reject.enabled = enabled;
		this.dismiss.enabled = enabled && !this.incomingId;
		if (value) {
			this.digits.disable();
		} else {
			this.digits.enable();
		}
		if (!value && !this.form.hidden && !this.element.hidden) {
			this.digits.focus();
		}
	}

	public override dispose(): void {
		this.revision++;
		super.dispose();
	}
}
