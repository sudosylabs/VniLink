import * as nls from '../../../nls.js';
import { $, addDisposableListener, append, EventType } from '../../../base/browser/dom.js';
import { Emitter } from '../../../base/common/event.js';
import { Disposable } from '../../../base/common/lifecycle.js';
import type { ISettingSchema } from '../../../platform/configuration/common/configurationSchema.js';
import { INativeRuntimeService } from '../../services/nativeRuntime/nativeRuntimeService.js';
import type { ISettingControl } from './settingRenderers.js';

/** `recording.directory`: a folder chosen in the system dialog, or the empty default. */
export class RecordingFolderControl extends Disposable implements ISettingControl {
	private readonly changed = this._register(new Emitter<unknown>());
	public readonly onDidChange = this.changed.event;
	private readonly path = $('span.recording-path');
	private readonly choose = $<HTMLButtonElement>('button.settings-button', { type: 'button' });
	private readonly reset = $<HTMLButtonElement>('button.settings-text-button', { type: 'button' });
	private readonly message = $('p.settings-help');
	private directory = '';
	private defaultDirectory = '';

	constructor(_key: string, _schema: ISettingSchema, @INativeRuntimeService private readonly runtime: INativeRuntimeService) {
		super();
	}

	public render(container: HTMLElement): void {
		const row = append(container, $('.recording-location'));
		this.choose.textContent = nls.localize('recording.chooseFolder', 'Choose folder');
		this.reset.textContent = nls.localize('recording.defaultFolder', 'Use default');
		row.append(this.path, this.choose, this.reset);
		container.append(this.message);
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
		this.message.textContent = '';
		try {
			const directory = await this.runtime.recordingDirectory('choose');
			if (!this.isDisposed() && directory) {
				this.changed.fire(directory);
			}
		} catch (error) {
			if (!this.isDisposed()) {
				this.message.textContent = error instanceof Error ? error.message : nls.localize('recording.folderFailed', 'Could not choose a recording folder.');
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
		this.path.textContent = this.directory || this.defaultDirectory;
	}

	private isDisposed(): boolean {
		return this._store.isDisposed;
	}
}
