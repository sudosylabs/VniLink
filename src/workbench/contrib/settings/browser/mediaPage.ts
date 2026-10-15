import { DisposableStore } from '../../../../base/common/lifecycle.js';
import { addDisposableListener } from '../../../../base/browser/domEvent.js';
import * as nls from '../../../../nls.js';
import { IConfigurationService, isMediaSources, type MediaSources } from '../../../../platform/configuration/common/configuration.js';
import type { NativeDevice } from '../../../../platform/engine/common/engine.js';
import { INativeRuntimeService } from '../../../services/nativeRuntime/common/nativeRuntimeService.js';
import { IPermissionService } from '../../../services/permissions/common/permissionService.js';
import { SettingsForm } from './settingsForm.js';
import { SettingsStateObserver } from './settingsStateObserver.js';

export class MediaPage extends SettingsForm<MediaSources> {
	private readonly permissionListeners = this._register(new DisposableStore());
	private readonly failures = new Map<keyof MediaSources, string>();
	private readonly groups = new Map<keyof MediaSources, HTMLSelectElement>();
	private devices: readonly NativeDevice[] = [];
	private readonly availability = new Map<keyof MediaSources, HTMLElement>();
	private permissionKey: string | undefined;
	private readonly permissionActions = document.createElement('div');

	constructor(@IConfigurationService private readonly configuration: IConfigurationService, @INativeRuntimeService private readonly runtime: INativeRuntimeService, @IPermissionService private readonly permissions: IPermissionService) {
		super('media', nls.localize('home.media', 'Audio & Camera'), nls.localize('media.description', 'Choose which cameras and audio sources this computer can share.'), nls.localize('media.note', 'Devices still need Camera, Microphone or System audio permission to use these sources.'));
		for (const [key, title, help] of [
			['cameras', nls.localize('media.cameras', 'Cameras'), nls.localize('media.camerasHelp', 'Choose the camera this computer shares with permitted devices.')],
			['microphones', nls.localize('media.microphones', 'Microphones'), nls.localize('media.microphonesHelp', 'Choose the microphone used when a permitted device requests it.')],
			['system', nls.localize('media.system', 'System audio'), nls.localize('media.systemHelp', 'Share this computer’s sound without changing its local speakers.')],
		] as const) {
			const section = this.section(title, help);
			const list = document.createElement('select');
			list.className = 'media-source-select';
			list.setAttribute('aria-label', title);
			const status = document.createElement('p');
			status.className = 'settings-help';
			section.append(list, status);
			this.availability.set(key, status);
			this.groups.set(key, list);
		}
		this.permissionActions.className = 'settings-apply-row';
		this.content.prepend(this.permissionActions);
		this._register(configuration.onDidChangeConfiguration(() => {
			void this.refresh();
		}));
		this._register(new SettingsStateObserver(this.element, permissions.onDidFocusWindow, () => this.refresh()));
		this.render(configuration.snapshot.values['media.sources']);
		void this.refresh();
	}

	protected async read(): Promise<MediaSources> {
		if (this.configuration.snapshot.error) {
			throw new Error(this.configuration.snapshot.error);
		}
		const [cameras, audio, permissions] = await Promise.allSettled([this.runtime.devices('camera'), this.runtime.devices('audio'), this.permissions.execute({ type: 'status' })]);
		if (this.disposed()) {
			return this.configuration.snapshot.values['media.sources'];
		}
		const previousDevices = JSON.stringify(this.devices);
		this.failures.clear();
		if (cameras.status === 'rejected') {
			this.failures.set('cameras', nls.localize('media.cameraFailed', 'Cameras could not be checked. Check system access and reconnect the device.'));
		}
		if (audio.status === 'rejected') {
			for (const key of ['microphones', 'system'] as const) {
				this.failures.set(key, nls.localize('media.audioFailed', 'Audio sources could not be checked. Check system access and audio devices.'));
			}
		}
		this.devices = [...(cameras.status === 'fulfilled' ? cameras.value : []), ...(audio.status === 'fulfilled' ? audio.value : [])];
		const permissionKey = JSON.stringify(permissions.status === 'fulfilled' ? permissions.value : null);
		if (permissionKey !== this.permissionKey) {
			this.permissionKey = permissionKey;
			this.permissionListeners.clear();
			this.permissionActions.replaceChildren();
			for (const permission of (permissions.status === 'fulfilled' ? permissions.value.permissions : []).filter(permission => ['camera', 'microphone'].includes(permission.kind) && permission.status !== 'granted' && (permission.canRequest || permission.canOpenSettings))) {
				const action = document.createElement('button');
				action.type = 'button';
				action.className = 'settings-text-button';
				action.textContent = permission.kind === 'camera' ? nls.localize('media.cameraAccess', 'Allow camera access') : nls.localize('media.microphoneAccess', 'Allow microphone access');
				this.permissionListeners.add(addDisposableListener(action, 'click', () => {
					void this.requestPermission(permission.kind, permission.canRequest);
				}));
				this.permissionActions.append(action);
			}
		}
		if (previousDevices !== JSON.stringify(this.devices)) {
			this.render(this.draft() ?? this.configuration.snapshot.values['media.sources']);
		}
		const saved = this.configuration.snapshot.values['media.sources'];
		return { cameras: [...saved.cameras].sort(), microphones: [...saved.microphones].sort(), system: [...saved.system].sort() };
	}

	private async requestPermission(kind: 'screen' | 'accessibility' | 'camera' | 'microphone', request: boolean): Promise<void> {
		try {
			await this.permissions.execute({ type: request ? 'request' : 'openSettings', kind });
			await this.refresh();
		} catch {
			if (!this.disposed()) {
				this.status.textContent = nls.localize('media.permissionFailed', 'Could not request system access. Check permissions in Access & Security.');
			}
		}
	}

	protected async write(value: MediaSources): Promise<MediaSources> {
		await this.configuration.updateValue({ key: 'media.sources', value });
		return this.read();
	}

	protected render(value: MediaSources): void {
		const defaults = { cameras: 'vnilink:auto-camera', microphones: 'vnilink:default-microphone', system: 'vnilink:default-system-audio' };
		for (const [key, select] of this.groups) {
			const kind = { cameras: 'camera', microphones: 'microphone', system: 'system' }[key];
			const devices = this.devices.filter(device => device.kind === kind);
			const automatic = defaults[key];
			const defaultName = devices.find(device => device.id === automatic)?.name ?? (key === 'cameras' ? nls.localize('media.autoCamera', 'Automatic (first available camera)') : nls.localize('media.default', 'System default'));
			const options = [{ id: automatic, name: defaultName }, { id: '', name: nls.localize('media.off', 'Off') }, ...devices.filter(device => device.id !== automatic)];
			const selected = value[key][0] ?? '';
			if (selected && !options.some(option => option.id === selected)) {
				options.push({ id: selected, name: nls.localize('media.missingDevice', 'Selected device (unavailable)') });
			}
			const signature = JSON.stringify(options);
			if (select.dataset.options !== signature) {
				select.replaceChildren(...options.map(device => {
					const option = document.createElement('option');
					option.value = device.id;
					option.textContent = device.name;
					return option;
				}));
				select.dataset.options = signature;
			}
			select.value = selected;
			const status = this.availability.get(key);
			if (status) {
				status.textContent = this.failures.get(key) ?? (devices.length ? '' : nls.localize('media.noDevices', 'No source is currently available. Check the device and system permissions.'));
			}
		}
	}

	protected draft(): MediaSources | undefined {
		const selected = (key: keyof MediaSources): string[] => {
			const value = this.groups.get(key)?.value;
			return value ? [value] : [];
		};
		const value = { cameras: selected('cameras'), microphones: selected('microphones'), system: selected('system') };
		return isMediaSources(value) ? value : undefined;
	}
}
