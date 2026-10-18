import * as nls from '../../../nls.js';
import { Emitter } from '../../../base/common/event.js';
import { Disposable } from '../../../base/common/lifecycle.js';
import { deepFreeze } from '../../../base/common/objects.js';
import type { PowerPolicy, RelayInput, SharingPolicy, SystemPolicy, SystemSupport, TerminalPolicy } from '../../engine/common/engine.js';
import { validateRelays } from '../../engine/common/relayValidation.js';
import type { EngineMainService } from '../../engine/electron-main/engineMainService.js';
import { settingSource, validateTerminalLimits, type ConfigurationSource } from '../common/configurationSchema.js';

type EngineSource = Exclude<ConfigurationSource, 'user'>;

interface PolicyState {
	readonly values: Readonly<Record<string, unknown>>;
	/** Settings this computer cannot honour. */
	readonly restricted: readonly string[];
}

/** How one engine policy maps to settings. */
interface PolicyBinding {
	read(engine: EngineMainService): Promise<PolicyState>;
	write(engine: EngineMainService, key: string, value: unknown, current: PolicyState): Promise<void>;
	readonly messages: { readonly unavailable: string; readonly readFailed: string; readonly writeFailed: string; readonly readbackFailed: string };
}

/** A change the policy's own rules refuse; reported as is rather than as a failed write. */
class SettingValidationError extends Error { }

const engineUnavailable = nls.localize('terminal.unavailable', 'The connection engine is unavailable. Restart VniLink to try again.');

async function expectDone(engine: EngineMainService, command: Parameters<EngineMainService['execute']>[0]): Promise<void> {
	const reply = await engine.execute(command);
	if (reply.type !== 'done') {
		throw new Error(`Engine refused ${command.type}`);
	}
}

function scaled(value: unknown, scale: number): number {
	return Math.round(Number(value) * scale);
}

const powerFields = [
	['power.incoming.keepSystemAwake', 'incoming_system'],
	['power.incoming.keepDisplayAwake', 'incoming_display'],
	['power.outgoing.keepSystemAwake', 'outgoing_system'],
	['power.outgoing.keepDisplayAwake', 'outgoing_display'],
	['power.lockAfterControl', 'lock_after_control'],
] as const;

const power: PolicyBinding = {
	async read(engine) {
		const reply = await engine.execute({ type: 'powerPolicy' });
		if (reply.type !== 'powerPolicy') {
			throw new Error('Invalid power policy reply');
		}
		const policy = reply.policy;
		return {
			values: {
				...Object.fromEntries(powerFields.map(([key, field]) => [key, policy[field]])),
				'power.incoming.idleTimeout': policy.incoming_idle_secs / 60,
				'power.outgoing.idleTimeout': policy.outgoing_idle_secs / 60,
			},
			restricted: [],
		};
	},
	async write(engine, key, value, current) {
		const values = { ...current.values, [key]: value };
		const policy: PowerPolicy = {
			incoming_system: values['power.incoming.keepSystemAwake'] === true,
			incoming_display: values['power.incoming.keepDisplayAwake'] === true,
			outgoing_system: values['power.outgoing.keepSystemAwake'] === true,
			outgoing_display: values['power.outgoing.keepDisplayAwake'] === true,
			lock_after_control: values['power.lockAfterControl'] === true,
			incoming_idle_secs: scaled(values['power.incoming.idleTimeout'], 60),
			outgoing_idle_secs: scaled(values['power.outgoing.idleTimeout'], 60),
		};
		await expectDone(engine, { type: 'setPowerPolicy', policy });
	},
	messages: {
		unavailable: engineUnavailable,
		readFailed: nls.localize('power.loadFailed', 'Power settings could not be loaded.'),
		writeFailed: nls.localize('settings.failed', 'Could not save these settings.'),
		readbackFailed: nls.localize('power.readbackFailed', 'The change was saved, but power settings could not be read again.'),
	},
};

const terminal: PolicyBinding = {
	async read(engine) {
		const reply = await engine.execute({ type: 'terminalPolicy' });
		if (reply.type !== 'terminalPolicy') {
			throw new Error('Invalid terminal policy reply');
		}
		const policy = reply.policy;
		return {
			values: {
				'terminal.maxSessions': policy.max_sessions,
				'terminal.maxSessionsPerDevice': policy.max_sessions_per_peer,
				'terminal.persistence': policy.allow_persistence,
				'terminal.detachedTimeout': policy.detached_timeout_secs,
				'terminal.historySize': policy.history_bytes / 1024,
				'terminal.replaySize': policy.replay_bytes / 1024,
			},
			restricted: [],
		};
	},
	async write(engine, key, value, current) {
		const values = { ...current.values, [key]: value };
		const invalid = validateTerminalLimits(values);
		if (invalid) {
			throw new SettingValidationError(invalid);
		}
		const policy: TerminalPolicy = {
			max_sessions: Number(values['terminal.maxSessions']),
			max_sessions_per_peer: Number(values['terminal.maxSessionsPerDevice']),
			allow_persistence: values['terminal.persistence'] === true,
			detached_timeout_secs: Number(values['terminal.detachedTimeout']),
			history_bytes: scaled(values['terminal.historySize'], 1024),
			replay_bytes: scaled(values['terminal.replaySize'], 1024),
		};
		await expectDone(engine, { type: 'setTerminalPolicy', policy });
	},
	messages: {
		unavailable: engineUnavailable,
		readFailed: nls.localize('terminal.loadFailed', 'Could not load terminal settings. Refresh to try again.'),
		writeFailed: nls.localize('terminal.saveFailed', 'Could not confirm the change. Close hosted terminals, then refresh before editing again.'),
		readbackFailed: nls.localize('terminal.saveFailed', 'Could not confirm the change. Close hosted terminals, then refresh before editing again.'),
	},
};

/** `sharing.screen` grants viewing and control together. */
const sharingFields: Readonly<Record<string, readonly (keyof SharingPolicy)[]>> = {
	'sharing.screen': ['view', 'control'],
	'sharing.files': ['files'],
	'sharing.terminal': ['terminal'],
	'sharing.camera': ['camera'],
	'sharing.microphone': ['microphone'],
	'sharing.systemAudio': ['system_audio'],
	'sharing.clipboard': ['clipboard'],
	'sharing.recording': ['recording'],
	'sharing.systemActions': ['system_actions'],
};

const sharing: PolicyBinding = {
	async read(engine) {
		const reply = await engine.execute({ type: 'sharingPolicy' });
		if (reply.type !== 'sharingPolicy') {
			throw new Error('Invalid sharing policy reply');
		}
		const policy = reply.policy;
		return { values: Object.fromEntries(Object.entries(sharingFields).map(([key, [field]]) => [key, policy[field]])), restricted: [] };
	},
	async write(engine, key, value) {
		// Partial updates patch the engine's consent atomically.
		await expectDone(engine, { type: 'setSharingPolicy', policy: Object.fromEntries((sharingFields[key] ?? []).map(field => [field, value === true])) });
	},
	messages: {
		unavailable: engineUnavailable,
		readFailed: nls.localize('sharing.readFailed', 'Sharing settings could not be loaded.'),
		writeFailed: nls.localize('sharing.writeFailed', 'Sharing settings could not be changed.'),
		readbackFailed: nls.localize('sharing.uncertain', 'The change was saved, but its current state could not be read. Waiting for updated settings.'),
	},
};

const securityFields = [
	['security.allowLock', 'allow_lock', 'lock'],
	['security.allowRestart', 'allow_restart', 'restart'],
	['security.allowShutdown', 'allow_shutdown', 'shutdown'],
	['security.allowPrivacy', 'allow_privacy', 'privacy'],
] as const satisfies readonly (readonly [string, keyof SystemPolicy, keyof SystemSupport])[];

const security: PolicyBinding = {
	async read(engine) {
		const reply = await engine.execute({ type: 'systemPolicy' });
		if (reply.type !== 'systemPolicy') {
			throw new Error('Invalid security policy reply');
		}
		const { policy, support } = reply;
		return {
			values: Object.fromEntries(securityFields.map(([key, field]) => [key, policy[field]])),
			restricted: securityFields.filter(([, , capability]) => !support[capability]).map(([key]) => key),
		};
	},
	async write(engine, key, value, current) {
		if (current.restricted.includes(key)) {
			throw new SettingValidationError(nls.localize('security.unsupportedAction', 'This action is not supported on this computer.'));
		}
		const values = { ...current.values, [key]: value };
		const policy: SystemPolicy = { allow_lock: values['security.allowLock'] === true, allow_restart: values['security.allowRestart'] === true, allow_shutdown: values['security.allowShutdown'] === true, allow_privacy: values['security.allowPrivacy'] === true };
		await expectDone(engine, { type: 'setSystemPolicy', policy });
	},
	messages: {
		unavailable: nls.localize('security.unavailable', 'Security policy is unavailable while the connection engine is stopped.'),
		readFailed: nls.localize('security.loadFailed', 'Could not load security policy. The policy will be checked again automatically.'),
		writeFailed: nls.localize('security.changeFailed', 'Could not confirm the change. Checking the current policy before enabling controls.'),
		readbackFailed: nls.localize('security.readbackFailed', 'Policy changed, but could not be reloaded. Checking the current policy before enabling controls.'),
	},
};

function relayInputs(value: unknown): RelayInput[] {
	return Array.isArray(value) ? value.map((relay: { url: string; authenticated: boolean; replacementToken?: string }) => ({ url: relay.url, authenticated: relay.authenticated, replacementToken: relay.replacementToken ?? '' })) : [];
}

const relays: PolicyBinding = {
	async read(engine) {
		const reply = await engine.execute({ type: 'savedRelays' });
		if (reply.type !== 'relays') {
			throw new Error('Invalid relay reply');
		}
		// Saved tokens never leave the engine; only whether a relay has one.
		return { values: { 'connection.relays': reply.relays.map(relay => ({ url: relay.url, authenticated: relay.authenticated })) }, restricted: [] };
	},
	async write(engine, _key, value, current) {
		const inputs = relayInputs(value);
		if (inputs.length) {
			const saved = relayInputs(current.values['connection.relays']);
			const issues = validateRelays(inputs, new Set(saved.filter(relay => relay.authenticated).map(relay => relay.url)));
			if (issues.length) {
				throw new SettingValidationError(issues[0].message);
			}
		}
		try {
			await expectDone(engine, inputs.length ? { type: 'saveCustomRelays', relays: inputs } : { type: 'setRelayPublic' });
		} finally {
			await engine.refreshRestartRequired().catch(() => { /* The relay change stands even when its restart state is unknown. */ });
		}
	},
	messages: {
		unavailable: nls.localize('connection.unavailable', 'Connection settings are unavailable because the connection engine could not start. Restart VniLink to try again.'),
		readFailed: nls.localize('connection.loadFailed', 'Could not load connection settings. Your saved configuration has not been changed.'),
		writeFailed: nls.localize('connection.saveFailed', 'Could not save. Your previous relay configuration is unchanged; check the relay details and try again.'),
		readbackFailed: nls.localize('connection.readbackFailed', 'Saved, but the updated settings could not be loaded. Reload settings before editing again.'),
	},
};

const bindings: Readonly<Record<EngineSource, PolicyBinding>> = { power, terminal, sharing, security, relays };

const engineSources: readonly EngineSource[] = ['power', 'terminal', 'sharing', 'security', 'relays'];

/**
 * Settings the engine persists. Each policy is read whole, and each change is a
 * serialized read-modify-write followed by a read-back, so the published value
 * is always what Rust holds. A policy that cannot be read publishes no values.
 */
export class EngineSettings extends Disposable {
	private readonly states = new Map<EngineSource, PolicyState>();
	private readonly errors = new Map<EngineSource, string>();
	private readonly queues = new Map<EngineSource, Promise<unknown>>();
	private readonly changed = this._register(new Emitter<void>());
	public readonly onDidChange = this.changed.event;

	constructor(private readonly engine: EngineMainService) {
		super();
		for (const source of engineSources) {
			this.errors.set(source, bindings[source].messages.unavailable);
		}
		this._register(engine.onDidChangeAvailability(() => {
			void this.refresh();
		}));
	}

	public get values(): Readonly<Record<string, unknown>> {
		return Object.fromEntries([...this.states.values()].flatMap(state => Object.entries(state.values)));
	}

	public get restricted(): readonly string[] {
		return [...this.states.values()].flatMap(state => state.restricted);
	}

	public get sourceErrors(): Readonly<Partial<Record<ConfigurationSource, string>>> {
		return Object.fromEntries(this.errors);
	}

	/** Reads every policy again; a policy that cannot be read publishes its reason instead of failing. */
	public async refresh(): Promise<void> {
		await Promise.allSettled(engineSources.map(source => this.queue(source, () => this.read(source))));
	}

	public update(key: string, value: unknown): Promise<void> {
		const source = settingSource(key);
		if (!source || source === 'user') {
			return Promise.reject(new Error(`${key} is not an engine setting`));
		}
		const binding = bindings[source];
		return this.queue(source, async () => {
			const current = await this.read(source);
			try {
				await binding.write(this.engine, key, value, current);
			} catch (error) {
				if (error instanceof SettingValidationError) {
					throw error;
				}
				// Publish what the engine now holds before reporting the failure.
				await this.read(source).catch(() => undefined);
				throw new Error(binding.messages.writeFailed, { cause: error });
			}
			try {
				await this.read(source);
			} catch (error) {
				throw new Error(binding.messages.readbackFailed, { cause: error });
			}
		});
	}

	private async read(source: EngineSource): Promise<PolicyState> {
		const binding = bindings[source];
		if (!this.engine.available) {
			this.publish(source, undefined, binding.messages.unavailable);
			throw new Error(binding.messages.unavailable);
		}
		try {
			const state = deepFreeze(await binding.read(this.engine));
			this.publish(source, state, undefined);
			return state;
		} catch (error) {
			this.publish(source, undefined, binding.messages.readFailed);
			throw new Error(binding.messages.readFailed, { cause: error });
		}
	}

	private publish(source: EngineSource, state: PolicyState | undefined, error: string | undefined): void {
		const before = JSON.stringify([this.states.get(source), this.errors.get(source)]);
		if (state) {
			this.states.set(source, state);
		} else {
			this.states.delete(source);
		}
		if (error) {
			this.errors.set(source, error);
		} else {
			this.errors.delete(source);
		}
		if (before !== JSON.stringify([state, error]) && !this._store.isDisposed) {
			this.changed.fire();
		}
	}

	private queue<T>(source: EngineSource, task: () => Promise<T>): Promise<T> {
		const result = (this.queues.get(source) ?? Promise.resolve()).then(task);
		this.queues.set(source, result.catch(() => undefined));
		return result;
	}
}
