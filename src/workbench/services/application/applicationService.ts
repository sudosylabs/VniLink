// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import { createDecorator } from '../../../platform/instantiation/common/instantiation.js';
import type { Event } from '../../../base/common/event.js';

export const IApplicationService = createDecorator<IApplicationService>('applicationService');

export interface IApplicationService {
	readonly _serviceBrand: undefined;
	readonly restartRequired: boolean;
	readonly onDidChangeRestartRequired: Event<void>;
	restart(): Promise<void>;
	openRelayDocumentation(): Promise<void>;
}
