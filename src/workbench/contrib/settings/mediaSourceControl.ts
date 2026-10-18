import * as nls from '../../../nls.js';
import { $, addDisposableListener, EventType } from '../../../base/browser/dom.js';
import { Emitter } from '../../../base/common/event.js';
import { Disposable } from '../../../base/common/lifecycle.js';
import type { ISettingSchema } from '../../../platform/configuration/common/configurationSchema.js';
import type { NativeDevice } from '../../../platform/engine/common/engine.js';
import { INativeRuntimeService } from '../../services/nativeRuntime/nativeRuntimeService.js';
import { createSelect, wrapSelect, type ISettingControl } from './settingRenderers.js';

interface MediaSourceKind {
	readonly devices: 'camera' | 'audio';
	readonly kind: NativeDevice['kind'];
	readonly automaticName: string;
	readonly failed: string;
}

const kinds = new Map<string, MediaSourceKind>(Object.entries({
	'media.camera': { devices: 'camera', kind: 'camera', automaticName: nls.localize('media.autoCamera', 'Automatic (first available camera)'), failed: nls.localize('media.cameraFailed', 'Cameras could not be checked. Check system access and reconnect the device.') },
	'media.microphone': { devices: 'audio', kind: 'microphone', automaticName: nls.localize('media.default', 'System default'), failed: nls.localize('media.audioFailed', 'Audio sources could not be checked. Check system access and audio devices.') },
	'media.systemAudio': { devices: 'audio', kind: 'system', automaticName: nls.localize('media.default', 'System default'), failed: nls.localize('media.audioFailed', 'Audio sources could not be checked. Check system access and audio devices.') },
}));

/**
 * One shared media source, chosen from the devices this computer has now.
 * The setting's default is the automatic source; the empty id turns it off.
 */
export class MediaSourceControl extends Disposable implements ISettingControl {
	private readonly changed = this._register(new Emitter<unknown>());
	public readonly onDidChange = this.changed.event;
	private readonly select = createSelect([]);
	private readonly availability = $('p.settings-help');
	private readonly source: MediaSourceKind;
	private devices: readonly NativeDevice[] = [];
	private failure: string | undefined;
	/** Whether this computer's devices have been read; until then no shortage is reported. */
	private loaded = false;
	private value = '';
	private signature = '';

	constructor(
		private readonly key: string,
		private readonly schema: ISettingSchema,
		@INativeRuntimeService private readonly runtime: INativeRuntimeService,
	) {
		super();
		const source = kinds.get(key);
		if (!source) {
			throw new Error(`${key} is not a media source`);
		}
		this.source = source;
	}

	public render(container: HTMLElement): void {
		this.select.className = 'media-source-select';
		this.select.dataset.setting = this.key;
		this.select.setAttribute('aria-label', this.schema.title ?? this.key);
		const picker = wrapSelect(this.select);
		picker.classList.add('media-source-picker');
		container.append(picker, this.availability);
		this._register(addDisposableListener(this.select, EventType.CHANGE, () => {
			this.changed.fire(this.select.value);
		}));
	}

	public async refresh(): Promise<void> {
		try {
			const devices = await this.runtime.devices(this.source.devices);
			this.devices = devices.filter(device => device.kind === this.source.kind);
			this.failure = undefined;
			this.loaded = true;
		} catch {
			this.failure = this.source.failed;
		}
		if (!this.isDisposed()) {
			this.renderOptions();
		}
	}

	public setValue(value: unknown): void {
		this.value = typeof value === 'string' ? value : '';
		this.renderOptions();
	}

	public setEnabled(enabled: boolean): void {
		this.select.disabled = !enabled;
	}

	private renderOptions(): void {
		const automatic = typeof this.schema.default === 'string' ? this.schema.default : '';
		const options = [
			{ id: automatic, name: this.devices.find(device => device.id === automatic)?.name ?? this.source.automaticName },
			{ id: '', name: nls.localize('media.off', 'Off') },
			...this.devices.filter(device => device.id !== automatic),
		];
		if (this.value && !options.some(option => option.id === this.value)) {
			options.push({ id: this.value, name: nls.localize('media.missingDevice', 'Selected device (unavailable)') });
		}
		const signature = JSON.stringify(options);
		if (signature !== this.signature) {
			this.signature = signature;
			this.select.replaceChildren(...Array.from(createSelect(options.map(option => [option.id, option.name])).options));
		}
		this.select.value = this.value;
		this.availability.textContent = this.failure ?? (this.devices.length || !this.loaded ? '' : nls.localize('media.noDevices', 'No source is currently available. Check the device and system permissions.'));
	}

	private isDisposed(): boolean {
		return this._store.isDisposed;
	}
}
