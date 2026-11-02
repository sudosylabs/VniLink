// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import { Emitter } from '../../../../base/common/event.js';
import { Disposable } from '../../../../base/common/lifecycle.js';
import type { ConfigurationSnapshot } from '../../common/configurationSnapshot.js';

/**
 * Main's configuration as its consumers read it: settings held in memory,
 * where `updateValue` saves a value and `setValue` changes one as an edit of
 * settings.json would, each announcing the new snapshot. Every update asked
 * for is kept in `updates`.
 */
export class TestConfigurationMainService extends Disposable {
	declare public readonly _serviceBrand: undefined;
	private readonly changed = this._register(new Emitter<ConfigurationSnapshot>());
	public readonly onDidChangeConfiguration = this.changed.event;
	public readonly updates: (readonly [key: string, value: unknown])[] = [];
	private readonly values: Record<string, unknown>;
	private revision = 0;

	constructor(values: Readonly<Record<string, unknown>> = {}, private readonly restricted: readonly string[] = []) {
		super();
		this.values = { ...values };
	}

	public get snapshot(): ConfigurationSnapshot {
		return { revision: this.revision, values: { ...this.values }, errors: {}, restricted: this.restricted };
	}

	public getValue(key: string): unknown {
		return this.values[key];
	}

	public updateValue(key: string, value: unknown): Promise<ConfigurationSnapshot> {
		this.updates.push([key, value]);
		this.setValue(key, value);
		return Promise.resolve(this.snapshot);
	}

	public setValue(key: string, value: unknown): void {
		this.values[key] = value;
		this.revision++;
		this.changed.fire(this.snapshot);
	}
}
