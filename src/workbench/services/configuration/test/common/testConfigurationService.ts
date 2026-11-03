// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import { Emitter } from '../../../../../base/common/event.js';
import { Disposable } from '../../../../../base/common/lifecycle.js';
import { ConfigurationTarget, type IConfigurationChangeEvent } from '../../../../../platform/configuration/common/configuration.js';
import { settingDefaults, settingKeys, settingSource, type ConfigurationSource } from '../../../../../platform/configuration/common/configurationSchema.js';
import type { IWorkbenchConfigurationService } from '../../configurationService.js';

/**
 * An {@link IWorkbenchConfigurationService} holding the settings in memory,
 * starting from their defaults, with the readability and support the main
 * process would report. Every update is kept in `updates`; `failure` refuses
 * updates with that error.
 */
export class TestConfigurationService extends Disposable implements IWorkbenchConfigurationService {
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

	/** Changes `key` as a change made elsewhere would, and announces it. */
	public set(key: string, value: unknown): void {
		this.values = { ...this.values, [key]: value };
		this.fire([key]);
	}

	/** Makes the settings of `source` unreadable with `error`, or readable again without one. */
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
