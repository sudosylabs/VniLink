import { app } from 'electron';
import { Emitter, Event } from '../../../base/common/event.js';
import { Disposable } from '../../../base/common/lifecycle.js';
import { createDecorator } from '../../instantiation/common/instantiation.js';
import { ILogService } from '../../log/common/log.js';

export const ILifecycleMainService = createDecorator<LifecycleMainService>('lifecycleMainService');

export interface ShutdownEvent {
	/** Delays quitting until `promise` settles. Failures are logged, never block shutdown. */
	join(id: string, promise: Promise<unknown>): void;
}

/** Exit code that asks the development runner to start the application again. */
export const DEVELOPMENT_RELAUNCH_EXIT_CODE = 42;

/**
 * Owns quitting and relaunching. Services that hold external resources join
 * {@link onWillShutdown} so the engine and settings close before the process exits.
 */
export class LifecycleMainService extends Disposable {
	declare public readonly _serviceBrand: undefined;
	private readonly willShutdown = this._register(new Emitter<ShutdownEvent>());
	public readonly onWillShutdown = this.willShutdown.event;
	private quitRequested = false;
	private shutdownComplete = false;
	private relaunching = false;
	private shutdown: Promise<void> | undefined;

	constructor(@ILogService private readonly logService: ILogService) {
		super();
		this._register(Event.fromNodeEventEmitter<Electron.Event>(app, 'before-quit')(event => {
			this.onBeforeQuit(event);
		}));
	}

	public get quitting(): boolean {
		return this.quitRequested;
	}

	/** Restarts the application. In development the runner restarts it after exit. */
	public relaunch(): void {
		if (this.relaunching) {
			return;
		}
		this.relaunching = true;
		if (!this.isDevelopmentRunner()) {
			app.relaunch();
		}
		app.quit();
	}

	/** Runs shutdown participants once; used directly when startup fails. */
	public runShutdown(): Promise<void> {
		this.quitRequested = true;
		this.shutdown ??= this.joinParticipants();
		return this.shutdown;
	}

	private onBeforeQuit(event: Electron.Event): void {
		if (this.shutdownComplete) {
			return;
		}
		event.preventDefault();
		if (this.shutdown) {
			return;
		}
		void this.runShutdown().finally(() => {
			this.shutdownComplete = true;
			if (this.relaunching && this.isDevelopmentRunner()) {
				app.exit(DEVELOPMENT_RELAUNCH_EXIT_CODE);
			} else {
				app.quit();
			}
		});
	}

	private async joinParticipants(): Promise<void> {
		const participants: { id: string; promise: Promise<unknown> }[] = [];
		this.willShutdown.fire({ join: (id, promise) => participants.push({ id, promise }) });
		const results = await Promise.allSettled(participants.map(participant => participant.promise));
		results.forEach((result, index) => {
			if (result.status === 'rejected') {
				this.logService.error(`Shutdown participant ${participants[index].id} failed`, result.reason);
			}
		});
	}

	private isDevelopmentRunner(): boolean {
		return !app.isPackaged && process.env.VNILINK_DEV === '1';
	}
}
