// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import { mkdir, open, rename, unlink } from 'node:fs/promises';
import { dirname } from 'node:path';
import { randomUUID } from 'node:crypto';
import * as nls from '../../../nls.js';
import { Disposable } from '../../../base/common/lifecycle.js';
import { Emitter } from '../../../base/common/event.js';
import { deepClone, deepFreeze, equals } from '../../../base/common/objects.js';
import { settingDefaults, settingSource, validateSetting } from '../common/configurationSchema.js';

/**
 * The `user` settings in settings.json. The file holds only settings that
 * differ from their defaults, plus keys this version does not know, which are
 * kept untouched. A change is published only after it reached the disk, and a
 * file that cannot be read is never overwritten.
 */
export class UserSettings extends Disposable {
	private file: Readonly<Record<string, unknown>> = {};
	private current: Readonly<Record<string, unknown>> = deepFreeze(settingDefaults('user'));
	private readError: string | undefined;
	private readonly changed = this._register(new Emitter<void>());
	public readonly onDidChange = this.changed.event;
	private pending: Promise<unknown> = Promise.resolve();
	private initialized = false;

	constructor(private readonly path: string) {
		super();
	}

	/** Every user setting: the saved value, or its default. */
	public get values(): Readonly<Record<string, unknown>> {
		return this.current;
	}

	public get error(): string | undefined {
		return this.readError;
	}

	public async initialize(): Promise<void> {
		if (this.initialized) {
			return;
		}
		try {
			const file = await open(this.path, 'r');
			try {
				if ((await file.stat()).size > 65536) {
					throw new Error('Settings file too large');
				}
				const contents: unknown = JSON.parse(await file.readFile('utf8'));
				if (typeof contents !== 'object' || contents === null || Array.isArray(contents)) {
					throw new Error('Invalid settings file');
				}
				const entries = Object.entries(contents);
				if (entries.some(([key, value]) => settingSource(key) !== undefined && (settingSource(key) !== 'user' || validateSetting(key, value) !== undefined))) {
					throw new Error('Invalid settings file');
				}
				this.accept(Object.fromEntries(entries));
			} finally {
				await file.close();
			}
		} catch (error: unknown) {
			if (!(typeof error === 'object' && error !== null && 'code' in error && error.code === 'ENOENT')) {
				// Preserve the file for recovery, and refuse to overwrite unknown data.
				this.readError = nls.localize('settings.fileInvalid', 'Settings could not be loaded. Correct settings.json and restart the app before saving.');
			}
		}
		this.initialized = true;
	}

	/** Saves one validated setting; the default value removes it from the file. */
	public update(key: string, value: unknown): Promise<void> {
		const captured: unknown = deepClone(value);
		const result = this.pending.then(async () => {
			if (!this.initialized || this._store.isDisposed || this.readError) {
				throw new Error(this.readError ?? nls.localize('settings.unavailable', 'Settings could not be loaded.'));
			}
			const others = Object.entries(this.file).filter(([saved]) => saved !== key);
			const file = Object.fromEntries(equals(captured, settingDefaults('user')[key]) ? others : [...others, [key, captured]]);
			await this.write(file);
			this.accept(file);
			this.changed.fire();
		});
		this.pending = result.catch(() => undefined);
		return result;
	}

	public async flush(): Promise<void> {
		await this.pending;
	}

	private accept(file: Record<string, unknown>): void {
		this.file = deepFreeze(file);
		const values = settingDefaults('user');
		for (const [key, value] of Object.entries(file)) {
			if (settingSource(key) === 'user') {
				values[key] = value;
			}
		}
		this.current = deepFreeze(deepClone(values));
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
