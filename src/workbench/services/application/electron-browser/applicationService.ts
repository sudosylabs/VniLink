import { Disposable, toDisposable } from '../../../../base/common/lifecycle.js';
import { Emitter } from '../../../../base/common/event.js';
import type { DesktopApi } from '../../../../platform/ipc/common/desktopApi.js';
import type { RestartState } from '../../../../platform/application/common/application.js';
import type { IApplicationService } from '../common/applicationService.js';

export class ApplicationService extends Disposable implements IApplicationService {
	declare public readonly _serviceBrand: undefined;
	private readonly changed = this._register(new Emitter<void>());
	public readonly onDidChangeRestartRequired = this.changed.event;
	private current: RestartState = { revision: -1, required: false };

	public get restartRequired(): boolean {
		return this.current.required;
	}

	constructor(private readonly api: Pick<DesktopApi, 'getRestartState' | 'onRestartStateChanged' | 'restartApplication' | 'openRelayDocumentation'>) {
		super();
		this._register(toDisposable(api.onRestartStateChanged(state => {
			this.accept(state);
		})));
		void api.getRestartState().then(state => {
			this.accept(state);
		}, () => { /* An unavailable engine cannot report restart requirements. */ });
	}

	private accept(state: RestartState): void {
		if (this._store.isDisposed || state.revision <= this.current.revision) {
			return;
		}
		const changed = state.required !== this.current.required;
		this.current = state;
		if (changed) {
			this.changed.fire();
		}
	}

	public restart(): Promise<void> {
		return this.api.restartApplication();
	}

	public openRelayDocumentation(): Promise<void> {
		return this.api.openRelayDocumentation();
	}
}
