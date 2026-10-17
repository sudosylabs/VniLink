import { InstantiationType, registerSingleton } from '../../../platform/instantiation/common/extensions.js';
import { Emitter } from '../../../base/common/event.js';
import { Disposable } from '../../../base/common/lifecycle.js';
import { IEngineService } from '../../../platform/engine/common/engineService.js';
import { INativeHostService } from '../../../platform/native/common/native.js';
import { IApplicationService } from './applicationService.js';

/** Relay documentation link shown beside relay settings. */
const relayDocumentation = 'https://docs.iroh.computer/relays/rate-limiting';

export class ApplicationService extends Disposable implements IApplicationService {
	declare public readonly _serviceBrand: undefined;
	private readonly changed = this._register(new Emitter<void>());
	public readonly onDidChangeRestartRequired = this.changed.event;

	constructor(
		@IEngineService private readonly engine: IEngineService,
		@INativeHostService private readonly nativeHostService: INativeHostService,
	) {
		super();
		this._register(engine.onDidChangeRestartRequired(() => {
			this.changed.fire();
		}));
	}

	public get restartRequired(): boolean {
		return this.engine.restartRequired;
	}

	public restart(): Promise<void> {
		return this.nativeHostService.relaunch();
	}

	public openRelayDocumentation(): Promise<void> {
		return this.nativeHostService.openExternal(relayDocumentation);
	}
}

registerSingleton(IApplicationService, ApplicationService, InstantiationType.Delayed);
