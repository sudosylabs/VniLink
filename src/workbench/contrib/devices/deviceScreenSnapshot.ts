import * as nls from '../../../nls.js';
import { Disposable, toDisposable } from '../../../base/common/lifecycle.js';
import type { DeviceSummary } from '../../services/devices/deviceService.js';
import type { ScreenSnapshotResult } from '../../services/screenSnapshot/screenSnapshot.js';
import type { IScreenSnapshotService } from '../../services/screenSnapshot/screenSnapshotService.js';

/** Still screenshot for the selected device. One request at a time; the image is not an input surface. */
export class DeviceScreenSnapshot extends Disposable {
	public readonly element = document.createElement('figure');
	private readonly frame = document.createElement('div');
	private readonly image = document.createElement('img');
	private readonly caption = document.createElement('figcaption');
	private readonly status = document.createElement('span');
	private readonly age = document.createElement('span');
	private readonly actions = document.createElement('div');
	private readonly refresh = document.createElement('button');
	private readonly consent = document.createElement('button');
	private endpointId: string | undefined;
	private deviceName = '';
	private presence: DeviceSummary['presence'] | 'unknown' | undefined;
	private epoch = 0;
	private url: string | undefined;
	private capturedUnixMs = 0;
	private state: ScreenSnapshotResult['status'] | 'idle' | 'loading' = 'idle';
	private request: AbortController | undefined;
	private watch: AbortController | undefined;
	private shown: { endpointId: string; sessionId: string; generation: string } | undefined;
	private statusText = '';

	constructor(private readonly snapshots: IScreenSnapshotService) {
		super();
		this.element.className = 'device-screen';
		this.frame.className = 'device-screen-frame';
		this.image.hidden = true;
		this.image.alt = '';
		this.status.className = 'device-screen-status';
		this.status.setAttribute('role', 'status');
		this.age.className = 'device-screen-age';
		this.caption.append(this.status, this.age);
		this.actions.className = 'device-screen-actions';
		this.refresh.type = 'button';
		this.refresh.className = 'device-button';
		this.refresh.textContent = nls.localize('devices.refreshScreen', 'Refresh screen');
		this.consent.type = 'button';
		this.consent.className = 'device-button';
		this.consent.hidden = true;
		this.actions.append(this.refresh, this.consent);
		this.frame.append(this.image);
		this.element.append(this.frame, this.caption, this.actions);
		this._register(toDisposable(() => {
			this.stop();
			this.revoke();
		}));
		const ageTimer = setInterval(() => {
			if (this.state === 'ready' && !this._store.isDisposed) {
				this.age.textContent = captureAge(this.capturedUnixMs, Date.now());
			}
		}, 15_000);
		const accessTimer = setInterval(() => {
			if (this.state === 'ready' && !this._store.isDisposed) {
				this.checkAccess();
			}
		}, 5_000);
		this._register(toDisposable(() => {
			clearInterval(ageTimer);
			clearInterval(accessTimer);
		}));
		this.refresh.addEventListener('click', () => {
			if (this.state !== 'loading') {
				this.begin(false);
			}
		});
		this.consent.addEventListener('click', () => {
			if (this.state !== 'loading') {
				this.begin(true);
			}
		});
		this.render();
	}

	public show(device: DeviceSummary | undefined): void {
		if (!device) {
			this.endpointId = undefined;
			this.shown = undefined;
			this.stop();
			this.revoke();
			this.epoch++;
			this.state = 'idle';
			this.element.hidden = true;
			this.render();
			return;
		}
		this.element.hidden = false;
		const same = device.id === this.endpointId;
		const wasConnected = this.presence === 'connected';
		this.deviceName = device.name;
		this.presence = device.presence;
		if (!same) {
			this.endpointId = device.id;
			this.shown = undefined;
			this.begin(false);
			return;
		}
		if (device.presence !== 'connected') {
			this.shown = undefined;
			this.stop();
			this.revoke();
			this.epoch++;
			this.state = 'disconnected';
			this.render();
			return;
		}
		if (!wasConnected) {
			this.begin(false);
			return;
		}
		if (this.state === 'ready') {
			this.checkAccess();
		}
		this.render();
	}

	private begin(prompt: boolean): void {
		this.stop();
		this.revoke();
		this.shown = undefined;
		const epoch = ++this.epoch;
		const endpointId = this.endpointId;
		if (!endpointId || this.presence !== 'connected') {
			this.state = 'disconnected';
			this.render();
			return;
		}
		this.state = 'loading';
		this.render();
		const request = new AbortController();
		this.request = request;
		void this.snapshots.capture(endpointId, request.signal, prompt).then(result => {
			if (this._store.isDisposed || epoch !== this.epoch) {
				return;
			}
			if (result.status === 'cancelled') {
				this.state = 'cancelled';
				this.render();
				return;
			}
			if (result.status === 'ready') {
				if (result.endpointId !== endpointId) {
					this.state = 'failed';
					this.render();
					return;
				}
				this.revoke();
				const bytes = new Uint8Array(result.jpeg.byteLength);
				bytes.set(result.jpeg);
				this.url = URL.createObjectURL(new Blob([bytes], { type: 'image/jpeg' }));
				this.capturedUnixMs = result.capturedUnixMs;
				this.image.src = this.url;
				this.image.hidden = false;
				this.image.alt = nls.localize('devices.screenshotName', 'Screenshot of {0}', this.deviceName);
				this.shown = { endpointId: result.endpointId, sessionId: result.sessionId, generation: result.generation };
			}
			this.state = result.status;
			this.render();
			if (this.state === 'ready') {
				this.checkAccess();
			}
		});
	}

	private checkAccess(): void {
		const endpointId = this.endpointId;
		const shown = this.shown;
		const epoch = this.epoch;
		if (!endpointId || !shown || this.state !== 'ready') {
			return;
		}
		this.watch?.abort();
		const watch = new AbortController();
		this.watch = watch;
		void this.snapshots.access(endpointId, watch.signal).then(access => {
			if (watch.signal.aborted || this._store.isDisposed || epoch !== this.epoch || this.state !== 'ready') {
				return;
			}
			if (access.status === 'denied') {
				this.clearShown('sharingDisabled');
				return;
			}
			if (access.status === 'disconnected' || access.status === 'unpaired') {
				this.clearShown('disconnected');
				return;
			}
			const replaced = access.status === 'replaced' || (access.status === 'allowed' && (access.endpointId !== shown.endpointId || access.sessionId !== shown.sessionId || access.generation !== shown.generation));
			if (!replaced) {
				return;
			}
			if (this.presence === 'connected') {
				this.begin(false);
			} else {
				this.clearShown('disconnected');
			}
		});
	}

	private clearShown(status: 'sharingDisabled' | 'disconnected'): void {
		this.stop();
		this.revoke();
		this.shown = undefined;
		this.epoch++;
		this.state = status;
		this.render();
	}

	private stop(): void {
		this.watch?.abort();
		this.watch = undefined;
		this.request?.abort();
		this.request = undefined;
	}

	private revoke(): void {
		if (this.url) {
			URL.revokeObjectURL(this.url);
			this.url = undefined;
		}
		this.image.removeAttribute('src');
		this.image.hidden = true;
		this.image.alt = '';
	}

	private render(): void {
		const connected = this.presence === 'connected';
		this.refresh.disabled = this.state === 'loading' || !connected;
		this.consent.disabled = this.state === 'loading' || !connected;
		this.frame.setAttribute('aria-busy', String(this.state === 'loading'));
		const asking = this.state === 'needsPermission' || this.state === 'needsSourceSelection';
		this.consent.hidden = !asking;
		if (this.state === 'needsPermission') {
			this.consent.textContent = nls.localize('devices.allowScreen', 'Allow screen capture');
		} else if (this.state === 'needsSourceSelection') {
			this.consent.textContent = nls.localize('devices.chooseScreen', 'Choose screen');
		}
		if (this.state === 'ready') {
			this.image.alt = nls.localize('devices.screenshotName', 'Screenshot of {0}', this.deviceName);
			this.age.textContent = captureAge(this.capturedUnixMs, Date.now());
		} else {
			this.age.textContent = '';
		}
		const status = this.state === 'ready' ? '' : this.message();
		if (status !== this.statusText) {
			this.statusText = status;
			this.status.textContent = status;
		}
	}

	private message(): string {
		switch (this.state) {
			case 'loading':
				return nls.localize('devices.screenLoading', 'Loading screen…');
			case 'disconnected':
			case 'idle':
				return nls.localize('devices.screenDisconnected', 'Connect to see this computer’s screen.');
			case 'sharingDisabled':
				return nls.localize('devices.screenSharingDisabled', 'Screen viewing is turned off on that computer.');
			case 'needsPermission':
				return nls.localize('devices.screenPermission', 'That computer has not allowed screen capture.');
			case 'needsSourceSelection':
				return nls.localize('devices.screenSource', 'Choose a screen on that computer, then refresh.');
			case 'unavailable':
				return nls.localize('devices.screenUnavailable', 'Screen unavailable.');
			case 'busy':
				return nls.localize('devices.screenBusy', 'That computer is busy. Try again in a moment.');
			case 'timedOut':
				return nls.localize('devices.screenTimedOut', 'The screen took too long to capture.');
			case 'replaced':
				return nls.localize('devices.screenReplaced', 'The connection changed. The previous screen was cleared.');
			case 'unpaired':
				return nls.localize('devices.screenUnpaired', 'This computer is no longer paired.');
			case 'unsupported':
				return nls.localize('devices.screenUnsupported', 'That computer cannot take a screenshot.');
			case 'failed':
			case 'cancelled':
				return nls.localize('devices.screenFailed', 'The screen could not be captured.');
			case 'ready':
				return '';
		}
	}
}

function captureAge(capturedUnixMs: number, now: number): string {
	const age = Math.max(0, now - capturedUnixMs);
	if (age < 10_000) {
		return nls.localize('devices.capturedNow', 'Screen captured just now.');
	}
	const seconds = Math.floor(age / 1000);
	if (seconds < 60) {
		return nls.localize('devices.capturedSeconds', 'Screen captured {0} seconds ago.', seconds);
	}
	const minutes = Math.floor(seconds / 60);
	if (minutes < 60) {
		return nls.localize('devices.capturedMinutes', 'Screen captured {0} minutes ago.', minutes);
	}
	return nls.localize('devices.capturedHours', 'Screen captured {0} hours ago.', Math.floor(minutes / 60));
}
