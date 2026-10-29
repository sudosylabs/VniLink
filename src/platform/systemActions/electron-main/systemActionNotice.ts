import * as nls from '../../../nls.js';
import { Disposable } from '../../../base/common/lifecycle.js';
import type { Event } from '../../../base/common/event.js';
import type { EngineCommand, EngineReply } from '../../engine/common/engine.js';
import type { EngineEventBatch } from '../../engine/common/engineService.js';
import { IEngineMainService, type EngineMainService } from '../../engine/electron-main/engineMainService.js';
import { createDecorator } from '../../instantiation/common/instantiation.js';
import type { NoticeContent } from '../common/notice.js';
import { windowPrompt } from './noticeWindow.js';
import { ILogService } from '../../log/common/log.js';

/** The engine as the notice uses it. */
export interface NoticeEngine {
	readonly available: boolean;
	readonly onDidChangeAvailability: Event<boolean>;
	readonly onDidReceiveEvents: Event<EngineEventBatch>;
	execute(command: EngineCommand): Promise<EngineReply>;
}

/** Asks the person at this computer; resolves with Cancel (0) or Now (1), or `undefined` once dismissed. */
export type NoticePrompt = (content: NoticeContent, signal: AbortSignal) => Promise<number | undefined>;

/** The button that cancels; the default, so Return or Escape keeps the computer running. */
const cancelButton = 0;

const nowButton = 1;

/**
 * Tells the person at this computer that a paired computer asked it to
 * restart or shut down, and lets them cancel or start it now. The notice is
 * the main process's own window, so it shows with no workbench open, and it
 * closes itself when the action ends another way.
 */
export class SystemActionNotices extends Disposable {
	private readonly open = new Map<string, AbortController>();

	constructor(
		private readonly engine: NoticeEngine,
		private readonly prompt: NoticePrompt,
		private readonly onError: (error: unknown) => void,
		private readonly now: () => number = Date.now,
	) {
		super();
		this._register(engine.onDidReceiveEvents(batch => {
			for (const event of batch.events) {
				if (event.type === 'systemActionScheduled') {
					void this.show(event.id, event.action, event.endpoint_id, event.remainingMs);
				} else if (event.type === 'systemActionEnded') {
					this.dismiss(event.id);
				}
			}
			if (batch.resync) {
				void this.resync();
			}
		}));
		this._register(engine.onDidChangeAvailability(available => {
			if (available) {
				void this.resync();
			}
		}));
		this._register({
			dispose: () => {
				for (const controller of this.open.values()) {
					controller.abort();
				}
				this.open.clear();
			},
		});
		if (engine.available) {
			void this.resync();
		}
	}

	/** Shows any countdown already running, such as one that began while events lapsed. */
	private async resync(): Promise<void> {
		try {
			const reply = await this.engine.execute({ type: 'scheduledSystemActions' });
			if (reply.type === 'scheduledSystemActions') {
				for (const action of reply.actions) {
					void this.show(action.id, action.action, action.endpoint_id, action.remainingMs);
				}
			}
		} catch (error) {
			this.onError(error);
		}
	}

	private async show(id: string, action: 'restart' | 'shutdown', endpointId: string, remainingMs: number): Promise<void> {
		if (this.open.has(id)) {
			return;
		}
		const controller = new AbortController();
		this.open.set(id, controller);
		const content: NoticeContent = { action, name: await this.nameOf(endpointId), deadline: this.now() + remainingMs };
		let response: number | undefined;
		try {
			response = controller.signal.aborted ? undefined : await this.prompt(content, controller.signal);
		} catch (error) {
			this.onError(error);
		}
		if (this.open.get(id) === controller) {
			this.open.delete(id);
		}
		// Ended another way while it showed: nothing is left to answer.
		if (controller.signal.aborted || response === undefined) {
			return;
		}
		try {
			if (response === nowButton) {
				await this.engine.execute({ type: 'runHostedSystemActionNow', id });
			} else if (response === cancelButton) {
				await this.engine.execute({ type: 'cancelHostedSystemAction', id });
			}
		} catch (error) {
			// It may have ended while its user chose; the engine keeps its outcome.
			this.onError(error);
		}
	}

	private dismiss(id: string): void {
		this.open.get(id)?.abort();
		this.open.delete(id);
	}

	private async nameOf(endpointId: string): Promise<string> {
		try {
			const reply = await this.engine.execute({ type: 'snapshot' });
			if (reply.type === 'snapshot') {
				const device = reply.snapshot.devices.find(candidate => candidate.endpoint_id === endpointId);
				const name = device?.pet_name ?? device?.hostname;
				if (name) {
					return name;
				}
			}
		} catch {
			// The generic name below still says what is happening.
		}
		return nls.localize('systemNotice.someone', 'A paired computer');
	}
}

export const ISystemActionNoticeMainService = createDecorator<SystemActionNoticeMainService>('systemActionNoticeMainService');

/** The notice for restarts and shutdowns asked of this computer, for the life of the application. */
export class SystemActionNoticeMainService extends Disposable {
	declare public readonly _serviceBrand: undefined;
	private started = false;

	constructor(
		@IEngineMainService private readonly engine: EngineMainService,
		@ILogService private readonly logService: ILogService,
	) {
		super();
	}

	public start(): void {
		if (this.started) {
			return;
		}
		this.started = true;
		this._register(new SystemActionNotices(this.engine, windowPrompt, error => {
			this.logService.error('System action notice failed.', error);
		}));
	}
}
