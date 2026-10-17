import { localize2 } from '../../nls.js';
import { Icons } from '../../base/common/icons.js';
import { ThemeIcon } from '../../base/common/themables.js';
import { Action2, registerAction2 } from '../../platform/actions/common/actions.js';
import { ContextKeyExpr } from '../../platform/contextkey/common/contextkey.js';
import type { ServicesAccessor } from '../../platform/instantiation/common/instantiation.js';
import { ActiveTabContext, RestartRequiredContext } from '../contextkeys.js';
import { HomeView } from '../contrib/home/homeView.js';
import { ViewActionsMenu } from '../parts/titlebar/viewActionsToolbar.js';
import { IApplicationService } from '../services/application/applicationService.js';

/** Restarts VniLink to apply saved settings the running engine cannot change live. */
class RestartAction extends Action2 {
	public static readonly ID = 'workbench.action.restart';

	constructor() {
		super({
			id: RestartAction.ID,
			title: localize2('app.restart', 'Restart VniLink'),
			icon: ThemeIcon.fromId(Icons.restart),
			precondition: RestartRequiredContext,
			menu: {
				id: ViewActionsMenu,
				group: 'navigation',
				order: 0,
				when: ContextKeyExpr.and(ActiveTabContext.isEqualTo(HomeView.ID), RestartRequiredContext),
			},
		});
	}

	public override run(accessor: ServicesAccessor): Promise<void> {
		return accessor.get(IApplicationService).restart();
	}
}

registerAction2(RestartAction);
