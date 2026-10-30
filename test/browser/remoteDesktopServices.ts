// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import type { IContextMenuDelegate } from '../../src/base/browser/contextmenu.js';
import type { IAction } from '../../src/base/common/actions.js';
import { Event } from '../../src/base/common/event.js';
import type { DisposableStore } from '../../src/base/common/lifecycle.js';
import { IMenuService } from '../../src/platform/actions/common/actions.js';
import { MenuService } from '../../src/platform/actions/common/menuService.js';
import { ICommandService } from '../../src/platform/commands/common/commands.js';
import { ContextKeyService } from '../../src/platform/contextkey/browser/contextKeyService.js';
import { IContextKeyService } from '../../src/platform/contextkey/common/contextkey.js';
import { IContextMenuService } from '../../src/platform/contextview/browser/contextView.js';
import type { RecordingStatus } from '../../src/platform/engine/common/engine.js';
import type { ServiceIdentifier } from '../../src/platform/instantiation/common/instantiation.js';
import { SyncDescriptor } from '../../src/platform/instantiation/common/descriptors.js';
import { ILogService, NullLogService } from '../../src/platform/log/common/log.js';
import type { UIStateKey, UIStateValues } from '../../src/platform/state/common/uiState.js';
import { CommandService } from '../../src/workbench/services/commands/commandService.js';
import { INativeRuntimeService } from '../../src/workbench/services/nativeRuntime/nativeRuntimeService.js';
import { ISystemActionService, type SystemPrivacyResult } from '../../src/workbench/services/systemActions/systemActionService.js';
import { IUIStateService } from '../../src/workbench/services/uiState/uiStateService.js';
import { FakeConfigurationService } from './settingsFixture.js';

/** A menu the view opened: from where, and what it offered. */
export interface ShownMenu {
	readonly anchor: unknown;
	readonly actions: readonly IAction[];
}

/** What a remote desktop tab asked of the services around it, for a test to read and steer. */
export class DesktopViewFakes {
	public readonly menus: ShownMenu[] = [];
	/** Privacy mode as each request left it; `privacyError` refuses the next one with these words. */
	public readonly privacyRequests: boolean[] = [];
	public privacyError: string | undefined;
	public readonly uiState = new Map<string, unknown>();
	/** Recordings started, and the status each reports, which a test moves on. */
	public readonly recordings = new Map<string, RecordingStatus>();
	public stopped: string[] = [];
	public released: string[] = [];
	private nextRecording = 0;

	/** The services, as a `ServiceCollection` takes them. */
	public services(disposables: DisposableStore): [ServiceIdentifier<unknown>, unknown][] {
		const configuration = disposables.add(new FakeConfigurationService());
		const privacy = (enabled: boolean): SystemPrivacyResult => ({ privacy: { displayPrivate: enabled, inputBlocked: enabled } });
		return [
			[IContextMenuService, {
				_serviceBrand: undefined, onDidShowContextMenu: Event.None, onDidHideContextMenu: Event.None,
				showContextMenu: (delegate: IContextMenuDelegate) => {
					this.menus.push({ anchor: delegate.getAnchor(), actions: delegate.getActions() });
					delegate.onHide?.(false);
				},
			}],
			[ISystemActionService, {
				_serviceBrand: undefined, onDidChange: Event.None, progress: () => undefined, run: () => Promise.resolve(), cancel: () => Promise.resolve(),
				privacy: () => Promise.resolve(privacy(this.privacyRequests.at(-1) ?? false)),
				setPrivacy: (_deviceId: string, enabled: boolean): Promise<SystemPrivacyResult> => {
					const error = this.privacyError;
					this.privacyError = undefined;
					if (error) {
						return Promise.resolve({ error });
					}
					this.privacyRequests.push(enabled);
					return Promise.resolve(privacy(enabled));
				},
			}],
			[IUIStateService, {
				_serviceBrand: undefined,
				get: <K extends UIStateKey>(key: K) => Promise.resolve(this.uiState.get(key) as UIStateValues[K] | undefined),
				set: <K extends UIStateKey>(key: K, value: UIStateValues[K] | undefined) => {
					this.uiState.set(key, value);
					return Promise.resolve();
				},
			}],
			[INativeRuntimeService, {
				_serviceBrand: undefined,
				startRecording: () => {
					const id = String(++this.nextRecording);
					const status: RecordingStatus = { id, phase: 'Recording', failure: null, frames: '0', bytes: '0', durationMs: '0', segments: 1 };
					this.recordings.set(id, status);
					return Promise.resolve(status);
				},
				recordingStatus: (id: string) => Promise.resolve(this.recordings.get(id)),
				stopRecording: (id: string) => {
					this.stopped.push(id);
					const status = this.recordings.get(id);
					const finished = status && { ...status, phase: 'Finished' as const };
					if (finished) {
						this.recordings.set(id, finished);
					}
					return Promise.resolve(finished);
				},
				releaseRecording: (id: string) => {
					this.released.push(id);
					return Promise.resolve();
				},
			}],
			[IContextKeyService, disposables.add(new ContextKeyService(configuration))],
			[ILogService, new NullLogService()],
			[ICommandService, new SyncDescriptor(CommandService)],
			[IMenuService, new SyncDescriptor(MenuService)],
		];
	}

	/** Starts a recording as the engine would, so the native runtime fakes follow it. */
	public startRecording(): Promise<{ readonly recording: string }> {
		const id = String(++this.nextRecording);
		this.recordings.set(id, { id, phase: 'Recording', failure: null, frames: '0', bytes: '0', durationMs: '0', segments: 1 });
		return Promise.resolve({ recording: id });
	}

	/** The labels of the last menu shown, separators as `-`. */
	public lastMenu(): string[] {
		return (this.menus.at(-1)?.actions ?? []).map(action => action.id === 'vs.actions.separator' ? '-' : action.label);
	}
}
