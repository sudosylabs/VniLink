import { mkdir, open, rename, unlink } from 'node:fs/promises';
import { dirname } from 'node:path';
import { randomUUID } from 'node:crypto';
import { Disposable } from '../../../base/common/lifecycle.js';
import { Emitter } from '../../../base/common/event.js';
import { freezeConfiguration, defaultConfiguration, isConfigurationValues, isConfigurationUpdate, type ConfigurationSnapshot, type ConfigurationUpdate } from '../common/configurationSnapshot.js';

/** Main-process owner. Disk commit precedes publication; failed writes keep old values. */
export class ConfigurationStore extends Disposable {
	private current: ConfigurationSnapshot = { revision: 0, values: defaultConfiguration, error: null };
	private readonly changed = this._register(new Emitter<ConfigurationSnapshot>());
	public readonly onDidChangeConfiguration = this.changed.event;
	private pending: Promise<unknown> = Promise.resolve();
	private initialized = false;

	constructor(private readonly path: string) {
		super();
	}

	public get snapshot(): ConfigurationSnapshot {
		return this.current;
	}

	public async initialize(): Promise<void> {
		if (this.initialized) {
			return;
		}
		try {
			const file = await open(this.path, 'r');
			try {
				if ((await file.stat()).size > 65536) {
					throw new Error('Configuration too large');
				}
				const contents: unknown = JSON.parse(await file.readFile('utf8'));
				if (typeof contents !== 'object' || contents === null || Array.isArray(contents)) {
					throw new Error('Invalid configuration');
				}
				const values: unknown = { ...defaultConfiguration, ...contents, ...('recording.defaults' in contents && typeof contents['recording.defaults'] === 'object' && contents['recording.defaults'] !== null ? { 'recording.defaults': { directory: '', ...contents['recording.defaults'] } } : {}) };
				if (!isConfigurationValues(values)) {
					throw new Error('Invalid configuration');
				}
				this.current = { revision: 0, values: freezeConfiguration(values), error: null };
			} finally {
				await file.close();
			}
		} catch (error: unknown) {
			if (!(typeof error === 'object' && error !== null && 'code' in error && error.code === 'ENOENT')) {
				// Preserve the file for recovery, and refuse to overwrite unknown data.
				this.current = { revision: 0, values: defaultConfiguration, error: 'Settings could not be loaded. Correct settings.json and restart the app before saving.' };
			}
		}
		this.initialized = true;
	}

	public updateValue(change: ConfigurationUpdate): Promise<ConfigurationSnapshot> {
		if (!isConfigurationUpdate(change)) {
			return Promise.reject(new Error('Invalid configuration update'));
		}
		const captured = freezeConfiguration({ ...this.current.values, [change.key]: change.value });
		const update = { key: change.key, value: captured[change.key] };
		const result = this.pending.then(async () => {
			if (!this.initialized || this._store.isDisposed || this.current.error) {
				throw new Error(this.current.error ?? 'Configuration unavailable');
			}
			const values = { ...this.current.values, [update.key]: update.value };
			await this.write(values);
			this.current = { revision: this.current.revision + 1, values: freezeConfiguration(values), error: null };
			this.changed.fire(this.current);
			return this.current;
		});
		this.pending = result.catch(() => undefined);
		return result;
	}

	public async flush(): Promise<void> {
		await this.pending;
	}

	private async write(values: unknown): Promise<void> {
		await mkdir(dirname(this.path), { recursive: true });
		const temporary = `${this.path}.${randomUUID()}.tmp`;
		const file = await open(temporary, 'wx', 0o600);
		try {
			try {
				await file.writeFile(JSON.stringify(values, null, 2) + '\n', 'utf8');
				await file.sync();
			} finally {
				await file.close();
			}
			await rename(temporary, this.path);
		} finally {
			await unlink(temporary).catch(() => undefined);
		}
	}
}
