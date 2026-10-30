// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import { createDecorator } from '../../../platform/instantiation/common/instantiation.js';

/** The relays the running engine uses; saved relays apply only after a restart. */
export interface RunningRelays {
	readonly publicRelay: boolean;
	readonly urls: readonly string[];
}

export const IConnectionService = createDecorator<IConnectionService>('connectionService');

export interface IConnectionService {
	readonly _serviceBrand: undefined;
	getRunningRelays(): Promise<RunningRelays>;
}
