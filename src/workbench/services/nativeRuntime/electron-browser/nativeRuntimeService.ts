import type { DesktopApi } from '../../../../platform/ipc/common/desktopApi.js';
import type { NativeDevice, PowerPolicy, PowerStatus, EngineCommand, NativeScreenSource, NativeSharingStatus, RecordingStatus, RecordingRetention } from '../../../../platform/engine/common/engine.js';
import type { INativeRuntimeService, NativeRuntime } from '../common/nativeRuntimeService.js';
/** Native backend facts and explicitly requested media operations. Peer grants remain separate. */
export class NativeRuntimeService implements INativeRuntimeService {
	declare public readonly _serviceBrand: undefined;
	private pending: Promise<unknown> = Promise.resolve();

	constructor(private readonly api: Pick<DesktopApi, 'engineCommand' | 'getCaptureDisplays'>, public readonly available: boolean) { }

	public async load(): Promise<NativeRuntime> {
		if (!this.available) {
			throw new Error('Native runtime unavailable');
		}
		const reply = await this.api.engineCommand({ type: 'nativeRuntime' });
		if (reply.type !== 'nativeRuntime') {
			throw new Error('Invalid native runtime reply');
		}
		return Object.freeze({ backends: Object.freeze(reply.backends.map(backend => Object.freeze({ ...backend }))), capabilities: Object.freeze(reply.capabilities.map(capability => Object.freeze({ ...capability }))) });
	}

	public async captureDisplays(): Promise<readonly NativeScreenSource[]> {
		const displays = await this.api.getCaptureDisplays();
		return Object.freeze(displays.map(display => Object.freeze({ ...display })));
	}

	public async startSharing(source: NativeScreenSource): Promise<readonly NativeSharingStatus[]> {
		await this.update({ type: 'startNativeScreenSharing', ...source });
		return this.sharingStatus();
	}

	public async stopSharing(): Promise<readonly NativeSharingStatus[]> {
		await this.update({ type: 'stopNativeScreenSharing' });
		return this.sharingStatus();
	}

	public async sharingStatus(): Promise<readonly NativeSharingStatus[]> {
		const reply = await this.api.engineCommand({ type: 'nativeSharingStatus' });
		if (reply.type !== 'nativeSharingStatus') {
			throw new Error('Invalid screen sharing reply');
		}
		return Object.freeze(reply.displays.map(display => Object.freeze({ ...display })));
	}

	public async remoteDevices(target: { readonly id: string; readonly generation: string }, kind: 'camera' | 'microphone' | 'system'): Promise<readonly NativeDevice[]> {
		const owner = { sessionId: target.id, generation: target.generation };
		const reply = await this.api.engineCommand(kind === 'camera' ? { type: 'remoteCameraDevices', ...owner } : { type: 'remoteAudioDevices', ...owner, kind: kind === 'microphone' ? 'Microphone' : 'System' });
		if (reply.type !== 'nativeDevices') {
			throw new Error('Invalid remote device reply');
		}
		return Object.freeze(reply.devices.map(device => Object.freeze({ ...device })));
	}

	private async recording(command: EngineCommand): Promise<Readonly<RecordingStatus>> {
		const reply = await this.api.engineCommand(command);
		if (reply.type !== 'recordingStatus') {
			throw new Error('Invalid recording reply');
		}
		return Object.freeze({ ...reply.status });
	}

	public startRecording(target: { readonly id: string; readonly generation: string }, options: { readonly destination: string; readonly maxBytes: string; readonly maxDurationMs: string; readonly retention?: RecordingRetention }): Promise<Readonly<RecordingStatus>> {
		return this.recording({ type: 'startRecording', sessionId: target.id, generation: target.generation, ...options });
	}

	public recordingStatus(id: string): Promise<Readonly<RecordingStatus>> {
		return this.recording({ type: 'recordingStatus', id });
	}

	public stopRecording(id: string): Promise<Readonly<RecordingStatus>> {
		return this.recording({ type: 'stopRecording', id });
	}

	public async releaseRecording(id: string): Promise<void> {
		const reply = await this.api.engineCommand({ type: 'releaseRecording', id });
		if (reply.type !== 'done') {
			throw new Error('Recording has not finalized');
		}
	}

	public async devices(kind: 'camera' | 'audio' | 'output'): Promise<readonly NativeDevice[]> {
		if (!this.available) {
			throw new Error('Native runtime unavailable');
		}
		const reply = await this.api.engineCommand({ type: kind === 'camera' ? 'nativeCameraDevices' : kind === 'audio' ? 'nativeAudioDevices' : 'nativeAudioOutputs' });
		if (reply.type !== 'nativeDevices') {
			throw new Error('Invalid native device reply');
		}
		return Object.freeze(reply.devices.map(device => Object.freeze({ ...device })));
	}

	private update(command: EngineCommand): Promise<NativeRuntime> {
		const result = this.pending.then(async () => {
			if (!this.available) {
				throw new Error('Native runtime unavailable');
			}
			const reply = await this.api.engineCommand(command);
			if (reply.type !== 'done') {
				throw new Error('Native backend update refused');
			}
			return this.load();
		});
		this.pending = result.catch(() => undefined);
		return result;
	}

	public approveCameras(ids: readonly string[]): Promise<NativeRuntime> {
		return this.update({ type: 'setNativeCameraSources', ids: [...ids] });
	}

	public approveAudio(microphones: readonly string[], system: readonly string[]): Promise<NativeRuntime> {
		return this.update({ type: 'setNativeAudioSources', microphones: [...microphones], system: [...system] });
	}

	public async powerPolicy(): Promise<Readonly<PowerPolicy>> {
		if (!this.available) {
			throw new Error('Native runtime unavailable');
		}
		const reply = await this.api.engineCommand({ type: 'powerPolicy' });
		if (reply.type !== 'powerPolicy') {
			throw new Error('Invalid power policy reply');
		}
		return Object.freeze({ ...reply.policy });
	}

	public savePowerPolicy(policy: PowerPolicy): Promise<Readonly<PowerPolicy>> {
		const draft = { ...policy };
		const result = this.pending.then(async () => {
			if (!this.available) {
				throw new Error('Native runtime unavailable');
			}
			const reply = await this.api.engineCommand({ type: 'setPowerPolicy', policy: draft });
			if (reply.type !== 'done') {
				throw new Error('Power policy update refused');
			}
			return this.powerPolicy();
		});
		this.pending = result.catch(() => undefined);
		return result;
	}

	public async powerStatus(): Promise<readonly PowerStatus[]> {
		if (!this.available) {
			throw new Error('Native runtime unavailable');
		}
		const reply = await this.api.engineCommand({ type: 'powerStatus' });
		if (reply.type !== 'powerStatus') {
			throw new Error('Invalid power status reply');
		}
		return Object.freeze(reply.statuses.map(status => Object.freeze({ ...status })));
	}
}
