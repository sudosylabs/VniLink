import * as nls from '../../../nls.js';
import { Emitter } from '../../../base/common/event.js';
import { Disposable } from '../../../base/common/lifecycle.js';
import { engineErrorCategory, type EngineCommand, type SystemActionKind, type SystemOperation } from '../../../platform/engine/common/engine.js';
import { IEngineService } from '../../../platform/engine/common/engineService.js';
import { InstantiationType, registerSingleton } from '../../../platform/instantiation/common/extensions.js';
import { activeControllerSession, type ControllerSession } from '../devices/controllerSession.js';
import { ISystemActionService, type SystemActionProgress, type SystemPrivacyResult } from './systemActionService.js';

/** How often a counting-down restart or shutdown is read again. */
const followInterval = 1000;

/** How long an ended action's outcome stays on its row. */
const outcomeTime = 8000;

/** What this computer is following on one device. */
interface Followed {
	readonly session: ControllerSession;
	readonly operation: SystemOperation;
	timer?: ReturnType<typeof setTimeout>;
}

/** What an operation's state means to the person who asked, in words. */
export function systemActionMessage(operation: Pick<SystemOperation, 'action' | 'state' | 'detail' | 'noticeRemainingMs'>): string {
	const seconds = Math.max(1, Math.ceil(operation.noticeRemainingMs / 1000));
	switch (operation.state) {
		case 'admitted':
			return operation.action === 'lock' ? nls.localize('system.locking', 'Locking…') : nls.localize('system.starting', 'Starting…');
		case 'scheduled':
			return operation.action === 'shutdown'
				? nls.localize('system.shuttingDownIn', 'Shutting down in {0} s', seconds)
				: nls.localize('system.restartingIn', 'Restarting in {0} s', seconds);
		case 'executed':
			return startedMessage(operation.action);
		case 'cancelled':
			switch (operation.detail) {
				case 'cancelled at that computer': return nls.localize('system.cancelledThere', 'Cancelled there');
				case 'the computer that asked disconnected': return nls.localize('system.cancelledDisconnected', 'Cancelled: disconnected');
				default: return nls.localize('system.cancelled', 'Cancelled');
			}
		case 'refused':
			return operation.detail === 'disabled by host policy'
				? nls.localize('system.turnedOff', 'Turned off there')
				: nls.localize('system.notAllowed', 'Not allowed there');
		case 'failed':
			return failedMessage(operation.action);
	}
}

function startedMessage(action: SystemActionKind): string {
	switch (action) {
		case 'lock': return nls.localize('system.locked', 'Locked');
		case 'restart': return nls.localize('system.restartStarted', 'Restart started');
		case 'shutdown': return nls.localize('system.shutdownStarted', 'Shutdown started');
	}
}

function failedMessage(action: SystemActionKind): string {
	switch (action) {
		case 'lock': return nls.localize('system.lockFailed', 'Couldn\'t lock');
		case 'restart': return nls.localize('system.restartFailed', 'Couldn\'t restart');
		case 'shutdown': return nls.localize('system.shutdownFailed', 'Couldn\'t shut down');
	}
}

/** Why asking failed before the other computer answered, in words. */
export function systemRequestMessage(error: unknown): string {
	switch (engineErrorCategory(error) ?? '') {
		case 'Unauthorized': return nls.localize('system.requestNotAllowed', 'Not allowed there');
		case 'NoSession':
		case 'StaleSession': return nls.localize('system.requestDisconnected', 'Not connected');
		// Reached, but still doing an earlier action or slow to answer.
		case 'Busy': return nls.localize('system.requestBusy', 'Busy, try again');
		case 'Timeout': return nls.localize('system.requestTimeout', 'No answer in time');
		// Reached, and it cannot do it, as a computer whose system refuses.
		case 'Unsupported': return nls.localize('system.requestUnsupported', 'Not possible there');
		default: return nls.localize('system.requestFailed', 'Couldn\'t reach it');
	}
}

export class SystemActionService extends Disposable implements ISystemActionService {
	declare public readonly _serviceBrand: undefined;
	private readonly changed = this._register(new Emitter<string>());
	public readonly onDidChange = this.changed.event;
	private readonly progresses = new Map<string, SystemActionProgress>();
	private readonly followed = new Map<string, Followed>();
	private readonly clearTimers = new Map<string, ReturnType<typeof setTimeout>>();

	constructor(
		@IEngineService private readonly engine: IEngineService,
	) {
		super();
		this._register({
			dispose: () => {
				for (const followed of this.followed.values()) {
					clearTimeout(followed.timer);
				}
				for (const timer of this.clearTimers.values()) {
					clearTimeout(timer);
				}
			},
		});
	}

	public progress(deviceId: string): SystemActionProgress | undefined {
		return this.progresses.get(deviceId);
	}

	public async run(deviceId: string, action: SystemActionKind): Promise<void> {
		if (this.progresses.get(deviceId)?.phase === 'scheduled' || this.progresses.get(deviceId)?.phase === 'asking') {
			return;
		}
		this.set(deviceId, { action, phase: 'asking', message: action === 'lock' ? nls.localize('system.locking', 'Locking…') : nls.localize('system.asking', 'Asking…') });
		const session = await activeControllerSession(this.engine, deviceId);
		if (!session) {
			this.end(deviceId, action, nls.localize('system.requestDisconnected', 'Not connected'));
			return;
		}
		try {
			const reply = await this.engine.execute({ type: 'systemAction', sessionId: session.id, generation: session.generation, action });
			if (reply.type !== 'systemOperation') {
				throw new Error('Invalid system action reply');
			}
			this.show(deviceId, session, reply.operation);
		} catch (error) {
			this.end(deviceId, action, systemRequestMessage(error));
		}
	}

	public async cancel(deviceId: string): Promise<void> {
		const followed = this.followed.get(deviceId);
		if (followed?.operation.state !== 'scheduled') {
			return;
		}
		try {
			const reply = await this.engine.execute({ type: 'cancelSystemAction', sessionId: followed.session.id, generation: followed.session.generation, id: followed.operation.id });
			if (reply.type === 'systemOperation') {
				this.show(deviceId, followed.session, reply.operation);
			}
		} catch (error) {
			this.end(deviceId, followed.operation.action, systemRequestMessage(error));
		}
	}

	public privacy(deviceId: string): Promise<SystemPrivacyResult> {
		return this.askPrivacy(deviceId, session => ({ type: 'systemPrivacy', sessionId: session.id, generation: session.generation }));
	}

	public setPrivacy(deviceId: string, enabled: boolean): Promise<SystemPrivacyResult> {
		return this.askPrivacy(deviceId, session => ({ type: 'setSystemPrivacy', sessionId: session.id, generation: session.generation, enabled }));
	}

	private async askPrivacy(deviceId: string, command: (session: ControllerSession) => EngineCommand): Promise<SystemPrivacyResult> {
		const session = await activeControllerSession(this.engine, deviceId);
		if (!session) {
			return { error: nls.localize('system.requestDisconnected', 'Not connected') };
		}
		try {
			const reply = await this.engine.execute(command(session));
			if (reply.type !== 'systemPrivacy') {
				throw new Error('Invalid system privacy reply');
			}
			return { privacy: reply.privacy };
		} catch (error) {
			// Another computer holding it is the busy case: say so rather than "try again".
			return { error: engineErrorCategory(error) === 'Busy' ? nls.localize('system.privacyHeld', 'Another computer has privacy mode on') : systemRequestMessage(error) };
		}
	}

	/** Shows where an operation stands, and follows it while it counts down. */
	private show(deviceId: string, session: ControllerSession, operation: SystemOperation): void {
		clearTimeout(this.followed.get(deviceId)?.timer);
		const message = systemActionMessage(operation);
		if (operation.state !== 'scheduled' && operation.state !== 'admitted') {
			this.followed.delete(deviceId);
			this.end(deviceId, operation.action, message);
			return;
		}
		const followed: Followed = { session, operation };
		this.followed.set(deviceId, followed);
		this.set(deviceId, { action: operation.action, phase: operation.state === 'scheduled' ? 'scheduled' : 'asking', message });
		followed.timer = setTimeout(() => {
			void this.follow(deviceId, followed);
		}, followInterval);
	}

	private async follow(deviceId: string, followed: Followed): Promise<void> {
		try {
			const reply = await this.engine.execute({ type: 'systemOperation', sessionId: followed.session.id, generation: followed.session.generation, id: followed.operation.id });
			if (this.followed.get(deviceId) === followed && reply.type === 'systemOperation') {
				this.show(deviceId, followed.session, reply.operation);
			}
		} catch {
			if (this.followed.get(deviceId) === followed) {
				this.followed.delete(deviceId);
				// A restart or shutdown that started takes the connection with
				// it; losing it earlier in the notice proves nothing either way.
				const started = followed.operation.state === 'admitted' || followed.operation.noticeRemainingMs <= 2 * followInterval;
				this.end(deviceId, followed.operation.action, started
					? startedMessage(followed.operation.action)
					: nls.localize('system.lost', 'Connection lost'));
			}
		}
	}

	private end(deviceId: string, action: SystemActionKind, message: string): void {
		this.set(deviceId, { action, phase: 'ended', message });
		const timer = setTimeout(() => {
			this.clearTimers.delete(deviceId);
			if (this.progresses.get(deviceId)?.phase === 'ended') {
				this.progresses.delete(deviceId);
				this.changed.fire(deviceId);
			}
		}, outcomeTime);
		this.clearTimers.set(deviceId, timer);
	}

	private set(deviceId: string, progress: SystemActionProgress): void {
		clearTimeout(this.clearTimers.get(deviceId));
		this.clearTimers.delete(deviceId);
		this.progresses.set(deviceId, progress);
		this.changed.fire(deviceId);
	}
}

registerSingleton(ISystemActionService, SystemActionService, InstantiationType.Delayed);
