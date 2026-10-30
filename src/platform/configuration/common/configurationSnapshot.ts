// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import { deepClone, deepFreeze } from '../../../base/common/objects.js';
import { configurationSources, settingSchema, validateSetting, type ConfigurationSource } from './configurationSchema.js';

/** Main-process IPC channel serving settings to windows. */
export const CONFIGURATION_CHANNEL_NAME = 'configuration';

/**
 * Every setting the main process can currently read, as one revision. A
 * setting whose source could not be read is absent from `values`, and its
 * source's message is in `errors`; `restricted` settings are readable but not
 * supported on this computer.
 */
export interface ConfigurationSnapshot {
	readonly revision: number;
	readonly values: Readonly<Record<string, unknown>>;
	readonly errors: Readonly<Partial<Record<ConfigurationSource, string>>>;
	readonly restricted: readonly string[];
}

export interface ConfigurationUpdate {
	readonly key: string;
	readonly value: unknown;
}

function record(value: unknown): value is Record<string, unknown> {
	return typeof value === 'object' && value !== null && !Array.isArray(value);
}

export function isConfigurationUpdate(value: unknown): value is ConfigurationUpdate {
	return record(value) && Object.keys(value).length === 2 && typeof value.key === 'string' && 'value' in value && validateSetting(value.key, value.value) === undefined;
}

/** Relay credentials are write-only: a snapshot carrying one is refused. */
function hasRelayToken(value: unknown): boolean {
	return Array.isArray(value) && value.some((relay: unknown) => record(relay) && 'replacementToken' in relay);
}

export function isConfigurationSnapshot(value: unknown): value is ConfigurationSnapshot {
	return record(value) && Object.keys(value).length === 4
		&& typeof value.revision === 'number' && Number.isSafeInteger(value.revision) && value.revision >= 0
		&& record(value.values)
		&& Object.entries(value.values).every(([key, setting]) => validateSetting(key, setting) === undefined && !hasRelayToken(setting))
		&& record(value.errors) && Object.entries(value.errors).every(([source, message]) => configurationSources.some(known => known === source) && typeof message === 'string')
		&& Array.isArray(value.restricted) && value.restricted.every((key: unknown) => typeof key === 'string' && settingSchema(key) !== undefined);
}

/** A window's private, immutable copy of a snapshot. */
export function freezeSnapshot(snapshot: ConfigurationSnapshot): ConfigurationSnapshot {
	return deepFreeze(deepClone(snapshot));
}
