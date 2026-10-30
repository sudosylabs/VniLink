/*---------------------------------------------------------------------------------------------
 *  Copyright (c) Microsoft Corporation. All rights reserved.
 *  Licensed under the MIT License. See LICENSE.vscode.txt in the project root for license information.
 *  Adapted from VS Code 1.141.0 src/vs/platform/secrets/common/secrets.ts.
 *--------------------------------------------------------------------------------------------*/
// ---------------------------------------------------------------------------------------------
// Modifications Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Modifications licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only AND MIT
// ---------------------------------------------------------------------------------------------

import { createDecorator } from '../../instantiation/common/instantiation.js';
import type { Event } from '../../../base/common/event.js';

export const SECRET_STORAGE_PREFIX = 'secret://';

export function secretStorageKey(key: string): string {
	return `${SECRET_STORAGE_PREFIX}${key}`;
}
/**
 * Serializes async work per key so concurrent secret reads and writes for the
 * same key cannot interleave. Adapted from VS Code's SequencerByKey.
 */
export class SequencerByKey {
	private readonly promises = new Map<string, Promise<unknown>>();

	public queue<T>(key: string, task: () => Promise<T>): Promise<T> {
		const pending = this.promises.get(key) ?? Promise.resolve();
		const next: Promise<T> = pending.then(task, task);
		this.promises.set(key, next);
		const release = (): void => {
			if (this.promises.get(key) === next) {
				this.promises.delete(key);
			}
		};
		next.then(release, release);
		return next;
	}
}

export async function readEncryptedSecret(
	key: string,
	storageGet: (fullKey: string) => string | undefined,
	decrypt: (value: string) => Promise<string>,
): Promise<string | undefined> {
	const encrypted = storageGet(secretStorageKey(key));
	if (!encrypted) {
		return undefined;
	}
	return await decrypt(encrypted);
}

export async function writeEncryptedSecret(
	key: string,
	value: string,
	storageSet: (fullKey: string, encrypted: string) => void,
	encrypt: (value: string) => Promise<string>,
): Promise<void> {
	const encrypted = await encrypt(value);
	storageSet(secretStorageKey(key), encrypted);
}

export const ISecretStorageService = createDecorator<ISecretStorageService>('secretStorageService');

export interface ISecretStorageService {
	readonly _serviceBrand: undefined;
	readonly onDidChangeSecret: Event<string>;
	get(key: string): Promise<string | undefined>;
	set(key: string, value: string): Promise<void>;
	delete(key: string): Promise<void>;
}
