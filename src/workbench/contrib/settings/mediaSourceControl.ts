// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import * as nls from '../../../nls.js';
import { $, addDisposableListener, EventType } from '../../../base/browser/dom.js';
import { isMacintosh } from '../../../base/common/platform.js';
import { Emitter } from '../../../base/common/event.js';
import { Disposable } from '../../../base/common/lifecycle.js';
import type { ISettingSchema } from '../../../platform/configuration/common/configurationSchema.js';
import type { NativeDevice } from '../../../platform/engine/common/engine.js';
import type { MachinePermission, PermissionKind } from '../../../platform/permissions/common/permissions.js';
import { INativeRuntimeService } from '../../services/nativeRuntime/nativeRuntimeService.js';
import { IPermissionService } from '../../services/permissions/permissionService.js';
import { createSelect, renderSettingRow, wrapSelect, type ISettingControl } from './settingRenderers.js';

interface MediaSourceKind {
	readonly devices: 'camera' | 'audio';
	readonly kind: NativeDevice['kind'];
	readonly automaticName: string;
	readonly failed: string;
	/** The system access this source needs, and what the row says until it is allowed. */
	readonly permission?: { readonly kind: PermissionKind; readonly missing: string };
}

const kinds = new Map<string, MediaSourceKind>(Object.entries({
	'media.camera': {
		devices: 'camera', kind: 'camera', automaticName: nls.localize('media.allCameras', 'All cameras'), failed: nls.localize('media.cameraFailed', 'Cameras could not be checked. Check system access and reconnect the device.'),
		permission: { kind: 'camera', missing: nls.localize('media.cameraMissing', 'Camera access isn’t allowed yet.') },
	},
	'media.microphone': {
		devices: 'audio', kind: 'microphone', automaticName: nls.localize('media.default', 'System default'), failed: nls.localize('media.audioFailed', 'Audio sources could not be checked. Check system access and audio devices.'),
		permission: { kind: 'microphone', missing: nls.localize('media.microphoneMissing', 'Microphone access isn’t allowed yet.') },
	},
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
	private readonly allow = $<HTMLButtonElement>('button.settings-button', { type: 'button', hidden: true });
	private help: HTMLElement | undefined;
	private readonly source: MediaSourceKind;
	/** The system access this source still needs, when it can be asked for. */
	private access: MachinePermission | undefined;
	private accessFailed = false;
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
		@IPermissionService private readonly permissionService: IPermissionService,
	) {
		super();
		const source = kinds.get(key);
		if (!source) {
			throw new Error(`${key} is not a media source`);
		}
		this.source = source;
	}

	public render(container: HTMLElement): void {
		const row = renderSettingRow(container, this.schema.title ?? this.key, this.schema.description, this.select);
		this.help = row.help;
		this.select.dataset.setting = this.key;
		this.allow.textContent = nls.localize('media.allow', 'Allow');
		row.control.append(this.allow, wrapSelect(this.select));
		this._register(addDisposableListener(this.select, EventType.CHANGE, () => {
			this.changed.fire(this.select.value);
		}));
		this._register(addDisposableListener(this.allow, EventType.CLICK, () => {
			void this.requestAccess();
		}));
	}

	public async refresh(): Promise<void> {
		await Promise.all([this.readDevices(), this.readAccess()]);
		if (!this.isDisposed()) {
			this.renderOptions();
		}
	}

	private async readDevices(): Promise<void> {
		try {
			const devices = await this.runtime.devices(this.source.devices);
			this.devices = devices.filter(device => device.kind === this.source.kind);
			this.failure = undefined;
			this.loaded = true;
		} catch {
			this.failure = this.source.failed;
		}
	}

	private async readAccess(): Promise<void> {
		const kind = this.source.permission?.kind;
		if (!kind) {
			return;
		}
		try {
			const permission = (await this.permissionService.execute({ type: 'status' })).permissions.find(candidate => candidate.kind === kind);
			this.access = permission && permission.status !== 'granted' && (permission.canRequest || permission.canOpenSettings) ? permission : undefined;
		} catch {
			// Access & Security reports system access it cannot check.
			this.access = undefined;
		}
	}

	private async requestAccess(): Promise<void> {
		const access = this.access;
		if (!access) {
			return;
		}
		this.accessFailed = false;
		try {
			await this.permissionService.execute({ type: access.canRequest ? 'request' : 'openSettings', kind: access.kind });
		} catch {
			this.accessFailed = true;
		}
		if (!this.isDisposed()) {
			await this.refresh();
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
		this.allow.hidden = !this.access;
		this.allow.textContent = this.access?.canRequest === false
			? (isMacintosh ? nls.localize('media.openSystemSettings', 'Open System Settings') : nls.localize('media.openSettings', 'Open Settings'))
			: nls.localize('media.allow', 'Allow');
		const problem = this.accessFailed ? nls.localize('media.permissionFailed', 'Could not request system access. Check permissions in Access & Security.')
			: this.access && this.source.permission ? this.source.permission.missing
				: this.failure ?? (this.devices.length || !this.loaded ? undefined : nls.localize('media.noDevices', 'No source is available. Check the device and system permissions.'));
		if (this.help) {
			this.help.textContent = problem ?? this.schema.description ?? '';
		}
	}

	private isDisposed(): boolean {
		return this._store.isDisposed;
	}
}
