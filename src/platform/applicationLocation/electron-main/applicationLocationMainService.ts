// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import * as nls from '../../../nls.js';
import { app, dialog } from 'electron';
import { isMacintosh } from '../../../base/common/platform.js';
import { isBuilt } from '../../environment/electron-main/environment.js';
import { createDecorator } from '../../instantiation/common/instantiation.js';
import { ILogService } from '../../log/common/log.js';
import { IStateService } from '../../state/node/state.js';
import type { ApplicationLocation } from '../common/applicationLocation.js';

export const IApplicationLocationMainService = createDecorator<ApplicationLocationMainService>('applicationLocationMainService');

/** The state key remembering that the person chose to keep the application where it is. */
const DECLINED_KEY = 'applicationLocation.moveDeclined';

/** What the service needs from Electron, replaced by tests. */
export interface ApplicationLocationEnvironment {
	/** A built macOS application, the only one that can be in the wrong folder. */
	readonly applies: boolean;
	isInApplicationsFolder(): boolean;
	/** Moves the bundle and relaunches it from there; whether it moved. */
	moveToApplicationsFolder(): boolean;
	/** Asks whether to move; whether the person chose to. */
	askToMove(): Promise<boolean>;
}

export const electronApplicationLocationEnvironment: ApplicationLocationEnvironment = {
	applies: isMacintosh && isBuilt(),
	isInApplicationsFolder: () => app.isInApplicationsFolder(),
	moveToApplicationsFolder: () => app.moveToApplicationsFolder({
		// A copy already there is replaced, unless it is the one running.
		conflictHandler: conflict => conflict === 'exists',
	}),
	askToMove: async () => {
		const { response } = await dialog.showMessageBox({
			type: 'question',
			message: nls.localize('applicationLocation.message', 'Move VniLink to the Applications folder?'),
			detail: nls.localize('applicationLocation.detail', 'macOS keeps the permissions you grant VniLink, such as screen recording, only while it stays in one place. It will restart from the Applications folder.'),
			buttons: [nls.localize('applicationLocation.move', 'Move to Applications'), nls.localize('applicationLocation.keep', 'Not Now')],
			defaultId: 0,
			cancelId: 1,
		});
		return response === 0;
	},
};

/**
 * Offers to move a macOS application started outside the Applications
 * folders there, once at startup; a person who keeps it where it is is not
 * asked again at startup, and can move it later from the welcome flow.
 */
export class ApplicationLocationMainService {
	constructor(
		private readonly environment: ApplicationLocationEnvironment,
		@IStateService private readonly stateService: IStateService,
		@ILogService private readonly logService: ILogService,
	) { }

	public get location(): ApplicationLocation {
		return { movable: this.environment.applies && !this.environment.isInApplicationsFolder() };
	}

	/** Asks once whether to move; whether the application is moving and about to relaunch. */
	public async offerAtStartup(): Promise<boolean> {
		if (!this.location.movable || this.stateService.getItem<boolean>(DECLINED_KEY, false)) {
			return false;
		}
		if (!await this.environment.askToMove()) {
			this.stateService.setItem(DECLINED_KEY, true);
			return false;
		}
		return this.move();
	}

	/** Moves the application and relaunches it from the Applications folder; whether it moved. */
	public move(): boolean {
		if (!this.location.movable) {
			return false;
		}
		try {
			return this.environment.moveToApplicationsFolder();
		} catch (error) {
			this.logService.error(error instanceof Error ? error : new Error(String(error)));
			return false;
		}
	}
}
