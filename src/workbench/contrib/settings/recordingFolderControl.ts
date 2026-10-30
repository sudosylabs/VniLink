// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import * as nls from '../../../nls.js';
import { $, addDisposableListener, EventType } from '../../../base/browser/dom.js';
import { Emitter } from '../../../base/common/event.js';
import { Disposable } from '../../../base/common/lifecycle.js';
import type { ISettingSchema } from '../../../platform/configuration/common/configurationSchema.js';
import { INativeRuntimeService } from '../../services/nativeRuntime/nativeRuntimeService.js';
import { renderSettingRow, type ISettingControl } from './settingRenderers.js';

/** `recording.directory`: a folder chosen in the system dialog, or the empty default. */
export class RecordingFolderControl extends Disposable implements ISettingControl {
	private readonly changed = this._register(new Emitter<unknown>());
	public readonly onDidChange = this.changed.event;
	private readonly choose = $<HTMLButtonElement>('button.settings-button', { type: 'button' });
	private readonly reset = $<HTMLButtonElement>('button.settings-text-button', { type: 'button' });
	/** The folder in use, shown as the row's help; a failure replaces it. */
	private path: HTMLElement | undefined;
	private failure = '';
	private directory = '';
	private defaultDirectory = '';

	constructor(private readonly key: string, private readonly schema: ISettingSchema, @INativeRuntimeService private readonly runtime: INativeRuntimeService) {
		super();
	}

	public render(container: HTMLElement): void {
		const row = renderSettingRow(container, this.schema.title ?? this.key, undefined);
		this.path = row.help;
		this.path.classList.add('recording-path');
		this.choose.textContent = nls.localize('recording.chooseFolder', 'Choose…');
		this.reset.textContent = nls.localize('recording.defaultFolder', 'Use default');
		row.control.append(this.reset, this.choose);
		this._register(addDisposableListener(this.choose, EventType.CLICK, () => {
			void this.chooseDirectory();
		}));
		this._register(addDisposableListener(this.reset, EventType.CLICK, () => {
			this.changed.fire('');
		}));
		void this.runtime.recordingDirectory('read').then(directory => {
			this.defaultDirectory = directory ?? '';
			this.renderPath();
		}, () => undefined);
	}

	private async chooseDirectory(): Promise<void> {
		this.failure = '';
		this.renderPath();
		try {
			const directory = await this.runtime.recordingDirectory('choose');
			if (!this.isDisposed() && directory) {
				this.changed.fire(directory);
			}
		} catch (error) {
			if (!this.isDisposed()) {
				this.failure = error instanceof Error ? error.message : nls.localize('recording.folderFailed', 'Could not choose a recording folder.');
				this.renderPath();
			}
		}
	}

	public setValue(value: unknown): void {
		this.directory = typeof value === 'string' ? value : '';
		this.renderPath();
	}

	public setEnabled(enabled: boolean): void {
		this.choose.disabled = this.reset.disabled = !enabled;
	}

	private renderPath(): void {
		if (this.path) {
			this.path.textContent = this.failure || this.directory || this.defaultDirectory;
		}
		// The default needs no reset.
		this.reset.hidden = !this.directory;
	}

	private isDisposed(): boolean {
		return this._store.isDisposed;
	}
}
