// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import type { Event } from '../../../base/common/event.js';
import { createDecorator } from '../../../platform/instantiation/common/instantiation.js';

/** The id of the permanent Home tab. */
export const HOME_TAB_ID = 'home';

export const IHomeService = createDecorator<IHomeService>('homeService');

/**
 * Brings a Home page forward for features that do not own Home, such as an
 * invitation the operating system opened, which shows on the Devices page.
 */
export interface IHomeService {
	readonly _serviceBrand: undefined;
	/** The page last asked for, which Home shows when it is created after the request. */
	readonly requestedPage: string | undefined;
	readonly onDidRequestPage: Event<string>;
	/** Selects the Home tab and shows the page registered as `id`. */
	showPage(id: string): void;
}
