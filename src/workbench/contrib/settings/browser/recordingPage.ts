import { INativeRuntimeService } from '../../../services/nativeRuntime/common/nativeRuntimeService.js';
import { addDisposableListener } from '../../../../base/browser/domEvent.js';
import * as nls from '../../../../nls.js';
import { IConfigurationService, isRecordingDefaults, type RecordingDefaults } from '../../../../platform/configuration/common/configuration.js';
import { SettingsForm } from './settingsForm.js';

/** Recorder limits are defaults; starting/stopping a recording belongs to its session. */
export class RecordingPage extends SettingsForm<RecordingDefaults> {
	private directory = '';
	private readonly location = document.createElement('span');
	private defaultDirectory = '';
	private readonly size: HTMLInputElement;
	private readonly duration: HTMLInputElement;
	private readonly segments: HTMLInputElement;
	private readonly storage: HTMLInputElement;

	constructor(@IConfigurationService private readonly configuration: IConfigurationService, @INativeRuntimeService private readonly runtime: INativeRuntimeService) {
		super('recording', nls.localize('home.recording', 'Recording'), nls.localize('recording.description', 'Set the limits used when you record a remote desktop session.'), nls.localize('recording.note', 'Used for new recordings. You can choose another destination when starting a recording.'));
		const destination = this.section(nls.localize('recording.destination', 'Save recordings'), nls.localize('recording.destinationHelp', 'Each recording gets its own file in this folder.'));
		const row = document.createElement('div');
		row.className = 'recording-location';
		this.location.className = 'recording-path';
		const browse = document.createElement('button');
		browse.type = 'button';
		browse.className = 'settings-button';
		browse.textContent = nls.localize('recording.chooseFolder', 'Choose folder');
		const reset = document.createElement('button');
		reset.type = 'button';
		reset.className = 'settings-text-button';
		reset.textContent = nls.localize('recording.defaultFolder', 'Use default');
		row.append(this.location, browse, reset);
		destination.append(row);
		this._register(addDisposableListener(browse, 'click', () => {
			void this.chooseDirectory();
		}));
		this._register(addDisposableListener(reset, 'click', () => {
			this.directory = '';
			this.location.textContent = this.defaultDirectory;
			this.update();
		}));
		const limits = this.section(nls.localize('recording.limits', 'Recording segments'), nls.localize('recording.limitsHelp', 'A new segment begins when a limit is reached or the video format changes.'));
		this.size = this.number(limits, 'maxBytes', nls.localize('recording.size', 'Maximum segment size'), 'MiB', 1, 1048576);
		this.duration = this.number(limits, 'maxDurationMs', nls.localize('recording.duration', 'Maximum segment duration'), nls.localize('recording.minutes', 'min'), 1, 1440);
		const retention = this.section(nls.localize('recording.retention', 'Retention per recording'), nls.localize('recording.retentionHelp', 'Older completed segments from the same recording are removed when these limits are exceeded. Unrelated files are never part of this retention rule.'));
		this.segments = this.number(retention, 'maxSegments', nls.localize('recording.segments', 'Keep up to'), nls.localize('recording.segmentUnit', 'segments'), 1, 65535);
		this.storage = this.number(retention, 'maxTotalBytes', nls.localize('recording.storage', 'Maximum combined size'), 'MiB', 1, 1048576);
		this._register(configuration.onDidChangeConfiguration(() => {
			void this.refresh();
		}));
		void this.refresh();
	}

	private async chooseDirectory(): Promise<void> {
		try {
			const directory = await this.runtime.recordingDirectory('choose');
			if (!this.disposed() && directory) {
				this.directory = directory;
				this.location.textContent = directory;
				this.update();
			}
		} catch (error) {
			if (!this.disposed()) {
				this.status.textContent = error instanceof Error ? error.message : nls.localize('recording.folderFailed', 'Could not choose a recording folder.');
			}
		}
	}

	protected async read(): Promise<RecordingDefaults> {
		if (!this.defaultDirectory) {
			this.defaultDirectory = await this.runtime.recordingDirectory('read') ?? '';
		}
		return this.configuration.snapshot.error ? Promise.reject(new Error(this.configuration.snapshot.error)) : Promise.resolve(this.configuration.snapshot.values['recording.defaults']);
	}

	protected async write(value: RecordingDefaults): Promise<RecordingDefaults> {
		await this.configuration.updateValue({ key: 'recording.defaults', value });
		return this.read();
	}

	protected render(value: RecordingDefaults): void {
		this.directory = value.directory;
		this.location.textContent = value.directory || this.defaultDirectory;
		this.size.value = String(Number(value.maxBytes) / 1048576);
		this.duration.value = String(Number(value.maxDurationMs) / 60000);
		this.segments.value = String(value.maxSegments);
		this.storage.value = String(Number(value.maxTotalBytes) / 1048576);
	}

	protected draft(): RecordingDefaults | undefined {
		const value = { directory: this.directory, maxBytes: String(Number(this.size.value) * 1048576), maxDurationMs: String(Number(this.duration.value) * 60000), maxSegments: Number(this.segments.value), maxTotalBytes: String(Number(this.storage.value) * 1048576) };
		return isRecordingDefaults(value) ? value : undefined;
	}
}
