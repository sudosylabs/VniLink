import { Barrier } from '../../../base/common/async.js';
import { Emitter, Event } from '../../../base/common/event.js';
import { Disposable } from '../../../base/common/lifecycle.js';
import { InstantiationType, registerSingleton } from '../../../platform/instantiation/common/extensions.js';
import { ILifecycleService, LifecyclePhase, StartupKind, type BeforeShutdownErrorEvent, type BeforeShutdownEvent, type WillShutdownEvent } from './lifecycle.js';

/**
 * Tracks the workbench's startup phases, in the role of VS Code's
 * `AbstractLifecycleService`. The window closes without vetoes, so the
 * shutdown events that support them never fire; `onDidShutdown` fires once
 * when the workbench goes away.
 */
export class LifecycleService extends Disposable implements ILifecycleService {
	declare public readonly _serviceBrand: undefined;
	public readonly startupKind = StartupKind.NewWindow;
	public readonly onBeforeShutdown = Event.None as Event<BeforeShutdownEvent>;
	public readonly onShutdownVeto = Event.None as Event<void>;
	public readonly onBeforeShutdownError = Event.None as Event<BeforeShutdownErrorEvent>;
	public readonly onWillShutdown = Event.None as Event<WillShutdownEvent>;
	private readonly didShutdown = this._register(new Emitter<void>());
	public readonly onDidShutdown = this.didShutdown.event;
	private readonly phaseBarriers = new Map<LifecyclePhase, Barrier>();
	private currentPhase = LifecyclePhase.Starting;
	private shutdownStarted = false;

	public get phase(): LifecyclePhase {
		return this.currentPhase;
	}

	public set phase(value: LifecyclePhase) {
		if (value < this.currentPhase) {
			throw new Error('Lifecycle cannot go backwards');
		}
		if (value === this.currentPhase) {
			return;
		}
		this.currentPhase = value;
		for (const [phase, barrier] of this.phaseBarriers) {
			if (phase <= value) {
				barrier.open();
				this.phaseBarriers.delete(phase);
			}
		}
	}

	public get willShutdown(): boolean {
		return this.shutdownStarted;
	}

	public async when(phase: LifecyclePhase): Promise<void> {
		if (phase <= this.currentPhase) {
			return;
		}
		let barrier = this.phaseBarriers.get(phase);
		if (!barrier) {
			barrier = new Barrier();
			this.phaseBarriers.set(phase, barrier);
		}
		await barrier.wait();
	}

	public shutdown(): Promise<void> {
		if (!this.shutdownStarted) {
			this.shutdownStarted = true;
			this.didShutdown.fire();
		}
		return Promise.resolve();
	}
}

registerSingleton(ILifecycleService, LifecycleService, InstantiationType.Eager);
