// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import { Emitter } from '../../../../../base/common/event.js';
import { Disposable } from '../../../../../base/common/lifecycle.js';
import type { SystemActionKind } from '../../../../../platform/engine/common/engine.js';
import type { ISystemActionService, SystemActionProgress, SystemPrivacyResult } from '../../systemActionService.js';

/** What the service was asked to do, in order. */
export type TestSystemActionCall =
	| { readonly type: 'run'; readonly deviceId: string; readonly action: SystemActionKind }
	| { readonly type: 'cancel'; readonly deviceId: string };

/**
 * An {@link ISystemActionService} whose restart and lock progress the test
 * sets with `set`. Actions and cancellations are kept in `calls`; privacy
 * mode follows what was last asked for.
 */
export class TestSystemActionService extends Disposable implements ISystemActionService {
	declare public readonly _serviceBrand: undefined;
	private readonly changed = this._register(new Emitter<string>());
	public readonly onDidChange = this.changed.event;
	private readonly progresses = new Map<string, SystemActionProgress>();
	public readonly calls: TestSystemActionCall[] = [];

	/** Shows `progress` for `deviceId`, or clears it. */
	public set(deviceId: string, progress: SystemActionProgress | undefined): void {
		if (progress) {
			this.progresses.set(deviceId, progress);
		} else {
			this.progresses.delete(deviceId);
		}
		this.changed.fire(deviceId);
	}

	public progress(deviceId: string): SystemActionProgress | undefined {
		return this.progresses.get(deviceId);
	}

	public run(deviceId: string, action: SystemActionKind): Promise<void> {
		this.calls.push({ type: 'run', deviceId, action });
		return Promise.resolve();
	}

	public cancel(deviceId: string): Promise<void> {
		this.calls.push({ type: 'cancel', deviceId });
		return Promise.resolve();
	}

	public privacy(): Promise<SystemPrivacyResult> {
		return Promise.resolve({ privacy: { displayPrivate: false, inputBlocked: false } });
	}

	public setPrivacy(_deviceId: string, enabled: boolean): Promise<SystemPrivacyResult> {
		return Promise.resolve({ privacy: { displayPrivate: enabled, inputBlocked: enabled } });
	}
}
