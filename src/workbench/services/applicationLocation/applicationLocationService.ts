// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import type { ApplicationLocation } from '../../../platform/applicationLocation/common/applicationLocation.js';
import { createDecorator } from '../../../platform/instantiation/common/instantiation.js';

export const IApplicationLocationService = createDecorator<IApplicationLocationService>('applicationLocationService');

/** Whether the macOS application runs outside the Applications folder, and moving it there. */
export interface IApplicationLocationService {
	readonly _serviceBrand: undefined;
	getLocation(): Promise<ApplicationLocation>;
	/** Moves the application, which relaunches from the Applications folder; whether it moved. */
	move(): Promise<boolean>;
}
