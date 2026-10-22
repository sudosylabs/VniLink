import { localize, localize2 } from '../../nls.js';
import { triggerDownload } from '../../base/browser/dom.js';
import { Icons } from '../../base/common/icons.js';
import { ThemeIcon } from '../../base/common/themables.js';
import { Action2, registerAction2 } from '../../platform/actions/common/actions.js';
import { ContextKeyExpr } from '../../platform/contextkey/common/contextkey.js';
import { IEngineService } from '../../platform/engine/common/engineService.js';
import type { ServicesAccessor } from '../../platform/instantiation/common/instantiation.js';
import { ActiveTabContext, EngineAvailableContext, RestartRequiredContext } from '../contextkeys.js';
import { HomeView } from '../contrib/home/homeView.js';
import { ViewActionsMenu } from '../parts/titlebar/viewActionsToolbar.js';
import { IApplicationService } from '../services/application/applicationService.js';
import { IDialogService } from '../services/dialogs/dialogService.js';

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

/** Tabs that show a paired computer's screen, where a picture's problems are seen. */
const desktopTab = ContextKeyExpr.regex(ActiveTabContext.key, /^desktop:/);

/**
 * Saves the engine's diagnostic snapshot as JSON: stages, phases, counts,
 * conditions and the last end, never screen, clipboard, terminal or
 * identity payload. The engine bounds it; the file is what it exported.
 */
class ExportDiagnosticsAction extends Action2 {
	public static readonly ID = 'workbench.action.exportDiagnostics';

	constructor() {
		super({
			id: ExportDiagnosticsAction.ID,
			title: localize2('app.exportDiagnostics', 'Export Diagnostics…'),
			icon: ThemeIcon.fromId(Icons.diagnostics),
			precondition: EngineAvailableContext,
			menu: {
				id: ViewActionsMenu,
				group: 'navigation',
				order: 90,
				when: desktopTab,
			},
		});
	}

	public override async run(accessor: ServicesAccessor): Promise<void> {
		const engine = accessor.get(IEngineService);
		const dialogs = accessor.get(IDialogService);
		let snapshot: string;
		try {
			const reply = await engine.execute({ type: 'diagnosticSnapshot' });
			if (reply.type !== 'diagnosticSnapshot') {
				throw new Error('Invalid diagnostic snapshot reply');
			}
			snapshot = reply.snapshot;
		} catch {
			await dialogs.show({
				type: 'error',
				message: localize('app.exportDiagnosticsFailed', 'VniLink couldn’t collect its diagnostics.'),
				detail: localize('app.exportDiagnosticsFailedDetail', 'The connection engine isn’t answering. Try again after it restarts.'),
				buttons: [localize('app.ok', 'OK')],
				cancelId: 0,
			});
			return;
		}
		triggerDownload(new TextEncoder().encode(snapshot), `VniLink-diagnostics-${new Date().toISOString().replace(/[:.]/g, '-')}.json`);
	}
}

registerAction2(ExportDiagnosticsAction);
