import { Emitter } from '../../src/base/common/event.js';
import { Disposable } from '../../src/base/common/lifecycle.js';
import { ConfigurationTarget, type IConfigurationChangeEvent, type IConfigurationData, type IConfigurationValue } from '../../src/platform/configuration/common/configuration.js';
import { settingDefaults, settingKeys, settingSource, type ConfigurationSource } from '../../src/platform/configuration/common/configurationSchema.js';
import type { NativeDevice, TerminalShells, VideoCodecCaps } from '../../src/platform/engine/common/engine.js';
import { InstantiationService } from '../../src/platform/instantiation/common/instantiationService.js';
import { ServiceCollection } from '../../src/platform/instantiation/common/serviceCollection.js';
import { permissionKinds, type PermissionCommand, type PermissionSnapshot } from '../../src/platform/permissions/common/permissions.js';
import { Registry } from '../../src/platform/registry/common/platform.js';
import { Extensions, type IHomePage, type IHomePagesRegistry } from '../../src/workbench/contrib/home/homePages.js';
import '../../src/workbench/contrib/settings/settings.contribution.js';
import { IApplicationService } from '../../src/workbench/services/application/applicationService.js';
import { IConnectionService, type RunningRelays } from '../../src/workbench/services/connection/connectionService.js';
import { IWorkbenchConfigurationService } from '../../src/workbench/services/configuration/configurationService.js';
import { INativeRuntimeService } from '../../src/workbench/services/nativeRuntime/nativeRuntimeService.js';
import { IPermissionService } from '../../src/workbench/services/permissions/permissionService.js';
import { IRemoteDesktopService } from '../../src/workbench/services/remoteDesktop/remoteDesktopService.js';
import { ITerminalHostService } from '../../src/workbench/services/terminal/terminalHostService.js';

export function assert(value: unknown, message: string): asserts value {
	if (!value) {
		throw new Error(message);
	}
}

export async function settle(milliseconds = 0): Promise<void> {
	await new Promise<void>(resolve => {
		setTimeout(resolve, milliseconds);
	});
}

/** Waits past the delay a typed value waits before it is applied. */
export function settleEdit(): Promise<void> {
	return settle(650);
}

export function edit(input: HTMLInputElement | HTMLSelectElement, value: string, event: 'input' | 'change' = input instanceof HTMLSelectElement ? 'change' : 'input'): void {
	input.value = value;
	input.dispatchEvent(new Event(event));
}

/** Reads a control again; assertions after an event must not reuse what TypeScript narrowed before it. */
export function current(control: HTMLInputElement | HTMLSelectElement): { readonly value: string; readonly disabled: boolean } {
	return { value: control.value, disabled: control.disabled };
}

export function toggle(input: HTMLInputElement): void {
	input.checked = !input.checked;
	input.dispatchEvent(new Event('change'));
}

/** Settings held in memory, with the readability and support the main process would report. */
export class FakeConfigurationService extends Disposable implements IWorkbenchConfigurationService {
	declare public readonly _serviceBrand: undefined;
	private readonly changed = this._register(new Emitter<IConfigurationChangeEvent>());
	public readonly onDidChangeConfiguration = this.changed.event;
	public values: Record<string, unknown> = settingDefaults();
	public readonly errors = new Map<ConfigurationSource, string>();
	public readonly restricted = new Set<string>();
	public readonly updates: { readonly key: string; readonly value: unknown }[] = [];
	public reloads = 0;
	/** Refuses an update with this error. */
	public failure: Error | undefined;

	public updateCount(): number {
		return this.updates.length;
	}

	public initialize(): Promise<void> {
		return Promise.resolve();
	}

	public getConfigurationData(): IConfigurationData | null {
		return null;
	}

	// eslint-disable-next-line @typescript-eslint/no-unnecessary-type-parameters
	public getValue<T>(key?: unknown): T {
		return (typeof key === 'string' ? this.values[key] : this.values) as T;
	}

	public updateValue(key: string, value: unknown): Promise<void> {
		this.updates.push({ key, value });
		if (this.failure) {
			return Promise.reject(this.failure);
		}
		// Relay credentials are write-only: the stored value never carries them.
		this.set(key, key === 'connection.relays' && Array.isArray(value) ? value.map((relay: { url: string; authenticated: boolean }) => ({ url: relay.url, authenticated: relay.authenticated })) : value);
		return Promise.resolve();
	}

	public set(key: string, value: unknown): void {
		this.values = { ...this.values, [key]: value };
		this.fire([key]);
	}

	public setError(source: ConfigurationSource, error: string | undefined): void {
		if (error) {
			this.errors.set(source, error);
		} else {
			this.errors.delete(source);
		}
		this.fire(settingKeys(source));
	}

	public setRestricted(key: string, restricted: boolean): void {
		if (restricted) {
			this.restricted.add(key);
		} else {
			this.restricted.delete(key);
		}
		this.fire([key]);
	}

	public inspect<T>(key: string): IConfigurationValue<Readonly<T>> {
		return { value: this.values[key] as T };
	}

	public reloadConfiguration(): Promise<void> {
		this.reloads++;
		return Promise.resolve();
	}

	public keys(): { default: string[]; policy: string[]; user: string[]; workspace: string[]; workspaceFolder: string[] } {
		return { default: settingKeys(), policy: [], user: [], workspace: [], workspaceFolder: [] };
	}

	public getUnavailableReason(key: string): string | undefined {
		const source = settingSource(key);
		return source ? this.errors.get(source) : undefined;
	}

	public isRestricted(key: string): boolean {
		return this.restricted.has(key);
	}

	private fire(keys: readonly string[]): void {
		this.changed.fire({ source: ConfigurationTarget.USER, affectedKeys: new Set(keys), change: { keys: [...keys], overrides: [] }, affectsConfiguration: section => keys.some(key => key === section || key.startsWith(`${section}.`)) });
	}
}

/** Operating-system access that the tests change between checks. */
export class FakePermissionService implements IPermissionService {
	declare public readonly _serviceBrand: undefined;
	public readonly focused = new Emitter<void>();
	public readonly onDidFocusWindow = this.focused.event;
	public readonly commands: PermissionCommand[] = [];
	public fail = false;
	public cameraAllowed = false;

	public execute(command: PermissionCommand): Promise<PermissionSnapshot> {
		this.commands.push(command);
		if (this.fail) {
			return Promise.reject(new Error('OS query failed'));
		}
		if (command.type === 'request') {
			this.cameraAllowed = true;
		}
		return Promise.resolve({
			development: true,
			permissions: permissionKinds.map(kind => ({
				kind,
				status: kind === 'camera' ? (this.cameraAllowed ? 'granted' : 'not-determined') : kind === 'screen' ? 'denied' : 'unsupported',
				canRequest: kind === 'camera' && !this.cameraAllowed,
				canOpenSettings: kind === 'camera' || kind === 'screen',
			})),
		});
	}
}

export const devices: readonly NativeDevice[] = [{ id: 'camera-one', name: 'Camera <one>', kind: 'camera' }, { id: 'mic-one', name: 'Microphone', kind: 'microphone' }];

export const desktopCodecs: VideoCodecCaps = {
	encode_baseline: false, encode_high: false, decode_baseline: true, decode_high: false, max_width: 3840, max_height: 2160, hardware: true, device: 'test', chroma444: false, additional: [
		{ codec: 'Vp9', encode: true, decode: true, hardware: false },
		{ codec: 'Av1', encode: false, decode: true, hardware: false },
		{ codec: 'H265', encode: true, decode: false, hardware: true },
	],
};

/** Builds Home's settings pages over fake services, as the workbench would. */
export class SettingsFixture extends Disposable {
	public readonly configuration = this._register(new FakeConfigurationService());
	public readonly permissions = new FakePermissionService();
	public chosenFolder: string | null = '/Recordings';
	public openedDocumentation = 0;
	public running: RunningRelays = { publicRelay: true, urls: [] };
	/** What this computer's engine reports it can offer; changing it models installing a shell. */
	public shells: TerminalShells = { shells: [{ id: '/bin/zsh', label: 'zsh' }, { id: '/bin/bash', label: 'bash' }], automatic: '/bin/zsh', current: '/bin/zsh' };
	private readonly instantiationService: InstantiationService;
	private readonly host = document.createElement('div');

	constructor() {
		super();
		const unused = (): Promise<never> => Promise.reject(new Error('Not exercised'));
		const runtime: INativeRuntimeService = {
			_serviceBrand: undefined, available: true, load: unused, captureDisplays: unused, startSharing: unused, stopSharing: unused, sharingStatus: unused,
			startRecording: unused, recordingStatus: unused, stopRecording: unused, releaseRecording: unused, remoteDevices: unused,
			recordingDirectory: action => Promise.resolve(action === 'read' ? '/Pictures/VniLink' : this.chosenFolder),
			devices: kind => Promise.resolve(devices.filter(device => kind === 'camera' ? device.kind === 'camera' : device.kind !== 'camera')),
			powerStatus: () => Promise.resolve([]),
		};
		const restart = new Emitter<void>();
		const services = new ServiceCollection(
			[IWorkbenchConfigurationService, this.configuration],
			[IPermissionService, this.permissions],
			[INativeRuntimeService, runtime],
			[IRemoteDesktopService, {
				_serviceBrand: undefined, available: true, codecs: () => Promise.resolve(desktopCodecs),
			}],
			[IApplicationService, {
				_serviceBrand: undefined, restartRequired: false, onDidChangeRestartRequired: restart.event, restart: () => Promise.resolve(),
				openRelayDocumentation: () => {
					this.openedDocumentation++;
					return Promise.resolve();
				}
			}],
			[IConnectionService, { _serviceBrand: undefined, getRunningRelays: () => Promise.resolve(this.running) }],
			[ITerminalHostService, { _serviceBrand: undefined, shells: () => Promise.resolve(this.shells) }],
		);
		this.instantiationService = this._register(new InstantiationService(services, true));
		this.host.className = 'home-content';
		document.body.append(this.host);
	}

	/** Creates the registered settings page `id` and shows it. */
	public open(id: string): IHomePage {
		const descriptor = Registry.as<IHomePagesRegistry>(Extensions.HomePages).getPage(id);
		assert(descriptor, `Settings page ${id} is not registered`);
		const page = this._register(this.instantiationService.createInstance<IHomePage>(descriptor.ctorDescriptor));
		this.host.append(page.element);
		return page;
	}

	public override dispose(): void {
		super.dispose();
		this.host.remove();
	}
}
