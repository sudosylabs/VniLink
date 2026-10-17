import { Disposable } from '../base/common/lifecycle.js';
import { IContextKeyService } from '../platform/contextkey/common/contextkey.js';
import { IEngineService } from '../platform/engine/common/engineService.js';
import type { IWorkbenchContribution } from './contributions.js';
import { ActiveTabContext, EngineAvailableContext, RestartRequiredContext } from './contextkeys.js';
import { ITabService } from './services/tabs/tabService.js';

/** Keeps the workbench context keys in step with the services that own the state. */
export class WorkbenchContextKeysHandler extends Disposable implements IWorkbenchContribution {
	public static readonly ID = 'workbench.contrib.contextKeys';

	constructor(
		@IContextKeyService contextKeyService: IContextKeyService,
		@ITabService tabService: ITabService,
		@IEngineService engineService: IEngineService,
	) {
		super();
		const activeTab = ActiveTabContext.bindTo(contextKeyService);
		const engineAvailable = EngineAvailableContext.bindTo(contextKeyService);
		const restartRequired = RestartRequiredContext.bindTo(contextKeyService);
		const update = (): void => {
			activeTab.set(tabService.activeTab?.id ?? '');
			engineAvailable.set(engineService.available);
			restartRequired.set(engineService.restartRequired);
		};
		update();
		this._register(tabService.onDidChange(update));
		this._register(engineService.onDidChangeAvailability(update));
		this._register(engineService.onDidChangeRestartRequired(update));
	}
}
