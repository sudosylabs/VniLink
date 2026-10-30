// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import * as nls from '../../../nls.js';
import { addDisposableListener, EventType } from '../../../base/browser/dom.js';
import { Emitter } from '../../../base/common/event.js';
import { Disposable } from '../../../base/common/lifecycle.js';
import type { ISettingSchema } from '../../../platform/configuration/common/configurationSchema.js';
import type { CodecPreference } from '../../../platform/engine/common/engine.js';
import { IRemoteDesktopService } from '../../services/remoteDesktop/remoteDesktopService.js';
import { createSelect, renderSettingRow, wrapSelect, type ISettingControl } from './settingRenderers.js';

const additionalCodecs = { H265: ['h265', 'H.265'], Vp8: ['vp8', 'VP8'], Vp9: ['vp9', 'VP9'], Av1: ['av1', 'AV1'] } as const satisfies Record<string, readonly [CodecPreference, string]>;

/** `desktop.codec`, offering only the codecs this computer can decode. */
export class CodecSettingControl extends Disposable implements ISettingControl {
	private readonly changed = this._register(new Emitter<unknown>());
	public readonly onDidChange = this.changed.event;
	private readonly select = createSelect([['auto', nls.localize('desktop.codecAuto', 'Automatic')]]);
	private help: HTMLElement | undefined;
	private value: unknown;
	private loaded = false;
	private enabled = false;

	constructor(
		private readonly key: string,
		private readonly schema: ISettingSchema,
		@IRemoteDesktopService private readonly desktopService: IRemoteDesktopService,
	) {
		super();
	}

	public render(container: HTMLElement): void {
		const row = renderSettingRow(container, this.schema.title ?? this.key, this.schema.description, this.select);
		this.help = row.help;
		this.select.dataset.setting = this.key;
		row.control.append(wrapSelect(this.select));
		this._register(addDisposableListener(this.select, EventType.CHANGE, () => {
			this.setMessage('');
			this.changed.fire(this.select.value);
		}));
		void this.load();
	}

	/** A problem replaces the help until it is resolved. */
	private setMessage(message: string): void {
		if (this.help) {
			this.help.textContent = message || (this.schema.description ?? '');
		}
	}

	private async load(): Promise<void> {
		try {
			const codecs = await this.desktopService.codecs();
			if (this.isDisposed()) {
				return;
			}
			const options: [string, string][] = [['auto', nls.localize('desktop.codecAuto', 'Automatic')]];
			if (codecs.decode_baseline || codecs.decode_high) {
				options.push(['h264', 'H.264']);
			}
			for (const codec of codecs.additional ?? []) {
				if (codec.decode) {
					options.push([...additionalCodecs[codec.codec]]);
				}
			}
			this.select.replaceChildren(...Array.from(createSelect(options).options));
			this.loaded = true;
			this.setValue(this.value);
		} catch {
			if (!this.isDisposed()) {
				this.setMessage(nls.localize('desktop.codecLoadFailed', 'Available codecs could not be checked.'));
			}
		} finally {
			if (!this.isDisposed()) {
				this.setEnabled(this.enabled);
			}
		}
	}

	public setValue(value: unknown): void {
		this.value = value;
		this.select.value = typeof value === 'string' ? value : '';
		if (this.loaded && !this.select.value) {
			this.setMessage(nls.localize('desktop.codecMissing', 'Your saved codec isn’t available on this computer. Choose Automatic or another codec.'));
		}
	}

	public setEnabled(enabled: boolean): void {
		this.enabled = enabled;
		this.select.disabled = !enabled || !this.loaded;
	}

	private isDisposed(): boolean {
		return this._store.isDisposed;
	}
}
