import * as nls from '../../../nls.js';
import { Disposable } from '../../../base/common/lifecycle.js';
import { RunOnceScheduler } from '../../../base/common/async.js';
import { addDisposableListener } from '../../../base/browser/domEvent.js';
import type { IDeviceService } from '../../services/devices/deviceService.js';

/** Owns the file/confirmation presentation, not pairing or authorization policy. */
export class DevicePairing extends Disposable {
	public readonly element = document.createElement('section');
	public readonly importButton = document.createElement('button');
	public readonly shareButton = document.createElement('button');
	private readonly file = document.createElement('input');
	private readonly heading = document.createElement('h2');
	private readonly description = document.createElement('p');
	private readonly code = document.createElement('output');
	private readonly form = document.createElement('form');
	private readonly digits = document.createElement('input');
	private readonly submit = document.createElement('button');
	private readonly reject = document.createElement('button');
	private readonly message = document.createElement('p');
	private readonly dismiss = document.createElement('button');
	private preview: Uint8Array | undefined;
	private incomingId: string | undefined;
	private expiresAt = 0;
	private awaitingEndpoint: string | undefined;
	private busy = false;
	private revision = 0;
	private readonly downloads = new Set<string>();
	private readonly polling = this._register(new RunOnceScheduler(() => void this.poll(), 500));

	constructor(private readonly devices: IDeviceService) {
		super();
		this.element.className = 'device-pairing';
		this.element.hidden = true;
		this.importButton.type = 'button';
		this.importButton.className = 'device-button primary';
		this.importButton.textContent = nls.localize('devices.import', 'Import invitation');
		this.shareButton.type = 'button';
		this.shareButton.className = 'device-button';
		this.shareButton.textContent = nls.localize('devices.share', 'Share this computer');
		this.file.type = 'file';
		this.file.accept = '.vnl';
		this.file.hidden = true;
		this.description.className = 'device-help';
		this.code.className = 'device-pairing-code';
		this.code.setAttribute('aria-label', nls.localize('devices.code', 'Confirmation code'));
		const label = document.createElement('label');
		label.textContent = nls.localize('devices.enterCode', 'Code shown on the other computer');
		this.digits.type = 'text';
		this.digits.inputMode = 'numeric';
		this.digits.autocomplete = 'off';
		this.digits.pattern = '[0-9]{3}';
		this.digits.maxLength = 3;
		this.digits.required = true;
		label.append(this.digits);
		this.submit.type = 'submit';
		this.submit.className = 'device-button primary';
		this.submit.textContent = nls.localize('devices.confirm', 'Confirm');
		this.reject.type = 'button';
		this.reject.className = 'device-button';
		this.reject.textContent = nls.localize('devices.reject', 'Reject');
		this.form.append(label, this.submit, this.reject);
		this.form.hidden = true;
		this.dismiss.type = 'button';
		this.dismiss.className = 'device-button';
		this.dismiss.textContent = nls.localize('devices.dismiss', 'Close');
		this.message.className = 'device-feedback';
		this.message.setAttribute('role', 'status');
		this.element.append(this.heading, this.description, this.code, this.form, this.message, this.dismiss, this.file);
		this._register(devices.onDidChangeDevices(known => {
			if (this.awaitingEndpoint && known.some(device => device.id === this.awaitingEndpoint && device.presence === 'connected')) {
				this.awaitingEndpoint = undefined;
				this.code.textContent = '';
				this.message.textContent = nls.localize('devices.paired', 'Computer paired. Your sharing settings still control what it can access.');
			}
		}));
		this.setBusy(false);
		this._register(addDisposableListener(this.importButton, 'click', () => {
			this.file.click();
		}));
		this._register(addDisposableListener(this.shareButton, 'click', () => {
			this.run(() => this.share());
		}));
		this._register(addDisposableListener(this.file, 'change', () => {
			const file = this.file.files?.[0];
			this.file.value = '';
			if (file) {
				this.run(() => this.open(file));
			}
		}));
		this._register(addDisposableListener(this.form, 'submit', event => {
			event.preventDefault();
			if (this.form.reportValidity()) {
				this.run(() => this.confirm());
			}
		}));
		this._register(addDisposableListener(this.reject, 'click', () => {
			this.run(async () => {
				await this.devices.rejectPairing();
				this.incomingId = undefined;
				this.form.hidden = true;
				this.message.textContent = nls.localize('devices.rejected', 'Pairing request rejected.');
			});
		}));
		this._register(addDisposableListener(this.dismiss, 'click', () => {
			this.revision++;
			this.preview = undefined;
			this.element.hidden = true;
			this.importButton.focus();
		}));
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
		if (file.size > 65536 || !file.size) {
			throw new Error(nls.localize('devices.fileSize', 'Choose a valid .vnl invitation smaller than 64 KB.'));
		}
		const bytes = new Uint8Array(await file.arrayBuffer());
		if (!this.current(revision)) {
			return;
		}
		this.preview = bytes;
		this.incomingId = undefined;
		this.form.hidden = false;
		this.reject.hidden = true;
		this.digits.focus();
	}

	private async share(): Promise<void> {
		const revision = this.begin(nls.localize('devices.shareTitle', 'Invite another computer'), nls.localize('devices.shareHelp', 'Send the .vnl file and share these digits with the other person. Keep VniLink open. The invitation expires in 15 minutes.'));
		const file = await this.devices.exportPairingFile();
		if (!this.current(revision)) {
			return;
		}
		for (const previous of this.downloads) {
			URL.revokeObjectURL(previous);
		}
		this.downloads.clear();
		const url = URL.createObjectURL(new Blob([new Uint8Array(file.bytes)], { type: 'application/octet-stream' }));
		this.downloads.add(url);
		const download = document.createElement('a');
		download.href = url;
		download.download = 'VniLink.vnl';
		this.element.append(download);
		download.click();
		download.remove();
		this.code.textContent = file.check;
		this.expiresAt = file.expiresAt;
		this.polling.schedule();
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
		if (!this.expiresAt || this.disposed()) {
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
			if (this.disposed() || this.busy || this.preview) {
				return;
			}
			if (request && request.endpoint_id !== this.incomingId) {
				this.begin(nls.localize('devices.approveTitle', 'Approve this computer'), nls.localize('devices.approveHelp', 'Ask the other person for the three digits on their screen before pairing.'));
				this.incomingId = request.endpoint_id;
				this.form.hidden = false;
				this.reject.hidden = false;
				this.setBusy(false);
			} else if (!request && this.incomingId) {
				this.incomingId = undefined;
				this.form.hidden = true;
				this.setBusy(false);
				this.message.textContent = nls.localize('devices.requestExpired', 'The pairing request ended. The other computer can try again.');
			}
		} catch {
			if (!this.disposed()) {
				this.message.textContent = nls.localize('devices.approvalUnavailable', 'Unable to check incoming pairing requests. Retrying…');
			}
		} finally {
			if (!this.disposed() && this.expiresAt) {
				this.polling.schedule();
			}
		}
	}

	private disposed(): boolean {
		return this._store.isDisposed;
	}

	private current(revision: number): boolean {
		return !this.disposed() && this.revision === revision;
	}

	private run(action: () => Promise<void>): void {
		if (this.busy || this.disposed()) {
			return;
		}
		this.setBusy(true);
		void action().catch((error: unknown) => {
			if (!this.disposed()) {
				this.element.hidden = false;
				this.message.textContent = error instanceof Error ? error.message : nls.localize('devices.pairingFailed', 'Pairing failed. Try again.');
			}
		}).finally(() => {
			if (!this.disposed()) {
				this.setBusy(false);
			}
		});
	}

	private setBusy(value: boolean): void {
		this.busy = value;
		this.element.setAttribute('aria-busy', String(value));
		for (const button of [this.importButton, this.shareButton, this.submit, this.reject, this.dismiss]) {
			button.disabled = value || !this.devices.available;
		}
		this.dismiss.disabled ||= !!this.incomingId;
		this.importButton.disabled ||= !!this.incomingId;
		this.shareButton.disabled ||= !!this.incomingId;
		this.digits.disabled = value;
		if (!value && !this.form.hidden && !this.element.hidden) {
			this.digits.focus();
		}
	}

	public override dispose(): void {
		this.revision++;
		for (const url of this.downloads) {
			URL.revokeObjectURL(url);
		}
		this.downloads.clear();
		super.dispose();
	}
}
