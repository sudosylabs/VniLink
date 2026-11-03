// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import { Disposable } from '../../../base/common/lifecycle.js';
import { IEngineMainService, type EngineMainService } from '../../engine/electron-main/engineMainService.js';
import { createDecorator } from '../../instantiation/common/instantiation.js';
import { ILifecycleMainService, type LifecycleMainService } from '../../lifecycle/electron-main/lifecycleMainService.js';
import { ILogService } from '../../log/common/log.js';
import type { NoticeContent } from '../../systemActions/common/notice.js';
import { windowPrompt } from '../../systemActions/electron-main/noticeWindow.js';
import { pairedComputerName, type NoticeEngine, type NoticePrompt } from '../../systemActions/electron-main/systemActionNotice.js';

/** The button that allows; Decline is the default, so Return or Escape declines. */
const allowButton = 1;

/**
 * Asks the person at this computer whether a paired computer may start
 * using what it shares, when sharing asks each time
 * (`sharing.unattended` off). The notice is the main process's own window,
 * so it shows with no workbench open; it closes itself when the request
 * ends another way, and declines at its deadline as the engine does.
 */
export class AdmissionNotices extends Disposable {
	private readonly open = new Map<string, AbortController>();
	private closed = false;

	constructor(
		private readonly engine: NoticeEngine,
		private readonly prompt: NoticePrompt,
		private readonly onError: (error: unknown) => void,
		private readonly now: () => number = Date.now,
	) {
		super();
		this._register(engine.onDidReceiveEvents(batch => {
			for (const event of batch.events) {
				if (event.type === 'admissionRequested') {
					void this.show(event.id, event.endpoint_id, event.remainingMs);
				} else if (event.type === 'admissionRequestEnded') {
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
				this.close();
			},
		});
		if (engine.available) {
			void this.resync();
		}
	}

	/** Closes every open notice, unanswered, and shows no more; the engine declines them at their deadline. */
	public close(): void {
		this.closed = true;
		for (const controller of this.open.values()) {
			controller.abort();
		}
		this.open.clear();
	}

	/** Shows any request already waiting, such as one that arrived while events lapsed. */
	private async resync(): Promise<void> {
		try {
			const reply = await this.engine.execute({ type: 'admissionRequests' });
			if (reply.type === 'admissionRequests') {
				for (const request of reply.requests) {
					void this.show(request.id, request.endpoint_id, request.remainingMs);
				}
			}
		} catch (error) {
			this.onError(error);
		}
	}

	private async show(id: string, endpointId: string, remainingMs: number): Promise<void> {
		if (this.closed || this.open.has(id)) {
			return;
		}
		const controller = new AbortController();
		this.open.set(id, controller);
		const content: NoticeContent = { action: 'admit', name: await pairedComputerName(this.engine, endpointId), deadline: this.now() + remainingMs };
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
			await this.engine.execute({ type: 'answerAdmissionRequest', id, accept: response === allowButton });
		} catch (error) {
			// It may have ended while its person chose; the engine keeps its outcome.
			this.onError(error);
		}
	}

	private dismiss(id: string): void {
		this.open.get(id)?.abort();
		this.open.delete(id);
	}
}

export const IAdmissionNoticeMainService = createDecorator<AdmissionNoticeMainService>('admissionNoticeMainService');

/** The question asked when a paired computer starts to use this one, for the life of the application. */
export class AdmissionNoticeMainService extends Disposable {
	declare public readonly _serviceBrand: undefined;
	private started = false;

	constructor(
		@IEngineMainService private readonly engine: EngineMainService,
		@ILifecycleMainService private readonly lifecycle: LifecycleMainService,
		@ILogService private readonly logService: ILogService,
	) {
		super();
	}

	public start(): void {
		if (this.started) {
			return;
		}
		this.started = true;
		const notices = this._register(new AdmissionNotices(this.engine, windowPrompt, error => {
			this.logService.error('Admission notice failed.', error);
		}));
		// Before the windows close: an open notice would cancel the quit.
		this._register(this.lifecycle.onWillShutdown(() => {
			notices.close();
		}));
	}
}
