import { localize, localize2 } from '../../nls.js';
import { triggerDownload } from '../../base/browser/dom.js';
import { Icons } from '../../base/common/icons.js';
import { ThemeIcon } from '../../base/common/themables.js';
import { Action2, registerAction2 } from '../../platform/actions/common/actions.js';
import { ContextKeyExpr } from '../../platform/contextkey/common/contextkey.js';
import type { EngineReply } from '../../platform/engine/common/engine.js';
import { IEngineService } from '../../platform/engine/common/engineService.js';
import type { ServicesAccessor } from '../../platform/instantiation/common/instantiation.js';
import { INativeHostService, isLogTails, type LogTail } from '../../platform/native/common/native.js';
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
 * Saves what explains a session in one JSON file: the engine's diagnostic
 * snapshot (stages, phases, counts, conditions and the last end), its flight
 * recorder's recent events and the end of its log file, and the end of this
 * run's main and window logs.
 * None holds screen, clipboard, terminal, typed text or identity payload;
 * the engine bounds what it exports, and the logs' home folder reads `~`.
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
		const nativeHost = accessor.get(INativeHostService);
		let snapshot: string;
		let recorder: string;
		try {
			const reply = await engine.execute({ type: 'diagnosticSnapshot' });
			if (reply.type !== 'diagnosticSnapshot') {
				throw new Error('Invalid diagnostic snapshot reply');
			}
			snapshot = reply.snapshot;
			const recorded = await engine.execute({ type: 'diagnosticRecorder' });
			if (recorded.type !== 'diagnosticRecorder') {
				throw new Error('Invalid diagnostic recorder reply');
			}
			recorder = recorded.events;
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
		// The logs are a help, not a requirement: an export without them still explains the engine.
		const engineLog = await engine.execute({ type: 'engineLogTail' }).then(engineLogTail, () => null);
		const logs: LogTail[] = await nativeHost.readLogTails().then(tails => isLogTails(tails) ? tails : [], () => []);
		const exported = {
			snapshot: JSON.parse(snapshot) as unknown,
			recorder: jsonLines(recorder),
			engineLog,
			logs,
		};
		triggerDownload(new TextEncoder().encode(JSON.stringify(exported, null, 1)), `VniLink-diagnostics-${new Date().toISOString().replace(/[:.]/g, '-')}.json`);
	}
}

function jsonLines(text: string): unknown[] {
	return text.split('\n').filter(line => line.length > 0).map(line => JSON.parse(line) as unknown);
}

/** The engine's log file, by its path with home as `~`, and its last events; `null` when it writes none. */
function engineLogTail(reply: EngineReply): { path: string; events: unknown[] } | null {
	if (reply.type !== 'engineLogTail' || reply.path === null) {
		return null;
	}
	try {
		return { path: reply.path, events: jsonLines(reply.text) };
	} catch {
		return null;
	}
}

/** Shows this run's logs folder, as VS Code's Open Logs Folder does. */
class OpenLogsFolderAction extends Action2 {
	public static readonly ID = 'workbench.action.openLogsFolder';

	constructor() {
		super({
			id: OpenLogsFolderAction.ID,
			title: localize2('app.openLogsFolder', 'Open Logs Folder'),
			icon: ThemeIcon.fromId(Icons.files),
			menu: {
				id: ViewActionsMenu,
				group: 'navigation',
				order: 91,
				when: desktopTab,
			},
		});
	}

	public override run(accessor: ServicesAccessor): Promise<void> {
		return accessor.get(INativeHostService).openLogsFolder();
	}
}

registerAction2(OpenLogsFolderAction);

registerAction2(ExportDiagnosticsAction);
