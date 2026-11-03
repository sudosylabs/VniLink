// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import { localize2 } from '../../../nls.js';
import { Icons } from '../../../base/common/icons.js';
import { Disposable } from '../../../base/common/lifecycle.js';
import { ThemeIcon } from '../../../base/common/themables.js';
import { Action2, registerAction2 } from '../../../platform/actions/common/actions.js';
import { ContextKeyExpr } from '../../../platform/contextkey/common/contextkey.js';
import { IInstantiationService, type ServicesAccessor } from '../../../platform/instantiation/common/instantiation.js';
import { ActiveTabContext, HomePageContext } from '../../contextkeys.js';
import { registerWorkbenchContribution2, WorkbenchPhase, type IWorkbenchContribution } from '../../contributions.js';
import { ViewActionsMenu } from '../../parts/titlebar/viewActionsToolbar.js';
import { HOME_TAB_ID } from '../../services/home/homeService.js';
import { ITabService } from '../../services/tabs/tabService.js';
import { IUIStateService } from '../../services/uiState/uiStateService.js';
import { OnboardingView } from './onboardingView.js';

/** Shows the welcome, or brings it forward when it is already open. */
function showOnboarding(tabService: ITabService, instantiationService: IInstantiationService): void {
	if (tabService.tabs.some(tab => tab.id === OnboardingView.ID)) {
		tabService.select(OnboardingView.ID);
	} else {
		tabService.open(instantiationService.createInstance(OnboardingView));
	}
}

/** Opens the welcome once Home is restored, until the person finishes or skips it. */
class OnboardingContribution extends Disposable implements IWorkbenchContribution {
	public static readonly ID = 'workbench.contrib.onboarding';

	constructor(
		@IUIStateService uiState: IUIStateService,
		@ITabService tabService: ITabService,
		@IInstantiationService instantiationService: IInstantiationService,
	) {
		super();
		void uiState.get('onboarding.completed').then(completed => {
			if (completed !== true && !this._store.isDisposed) {
				showOnboarding(tabService, instantiationService);
			}
		}, () => undefined);
	}
}

/** Runs the welcome again, from Home's General page, starting from the current settings. */
class ShowOnboardingAction extends Action2 {
	public static readonly ID = 'vnilink.onboarding.show';

	constructor() {
		super({
			id: ShowOnboardingAction.ID,
			title: localize2('onboarding.showAgain', 'Set Up VniLink Again'),
			icon: ThemeIcon.fromId(Icons.refresh),
			menu: {
				id: ViewActionsMenu,
				group: 'navigation',
				order: 10,
				when: ContextKeyExpr.and(ActiveTabContext.isEqualTo(HOME_TAB_ID), HomePageContext.isEqualTo('general')),
			},
		});
	}

	public override run(accessor: ServicesAccessor): void {
		showOnboarding(accessor.get(ITabService), accessor.get(IInstantiationService));
	}
}

registerWorkbenchContribution2(OnboardingContribution.ID, OnboardingContribution, WorkbenchPhase.AfterRestored);

registerAction2(ShowOnboardingAction);
