// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import { Disposable } from '../../../base/common/lifecycle.js';
import { IInstantiationService } from '../../../platform/instantiation/common/instantiation.js';
import { registerWorkbenchContribution2, WorkbenchPhase, type IWorkbenchContribution } from '../../contributions.js';
import { ITabService } from '../../services/tabs/tabService.js';
import { HomeView } from './homeView.js';

/** Opens the permanent Home tab before the workbench is shown. */
class HomeContribution extends Disposable implements IWorkbenchContribution {
	public static readonly ID = 'workbench.contrib.home';

	constructor(
		@ITabService tabService: ITabService,
		@IInstantiationService instantiationService: IInstantiationService,
	) {
		super();
		tabService.open(instantiationService.createInstance(HomeView));
	}
}

registerWorkbenchContribution2(HomeContribution.ID, HomeContribution, WorkbenchPhase.BlockRestore);
