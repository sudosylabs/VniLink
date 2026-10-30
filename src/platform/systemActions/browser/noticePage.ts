// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import '../../theme/browser/media/colors.css';
import './media/notice.css';
import * as nls from '../../../nls.js';
import { $, addDisposableListener, append, EventType } from '../../../base/browser/dom.js';
import { Button } from '../../../base/browser/ui/button/button.js';
import { renderIcon } from '../../../base/browser/ui/icon/icon.js';
import { Icons } from '../../../base/common/icons.js';
import { Disposable, toDisposable } from '../../../base/common/lifecycle.js';
import { defaultButtonStyles } from '../../theme/browser/defaultStyles.js';
import type { WindowBackgroundMaterial } from '../../window/common/windowMaterial.js';
import type { NoticeAnswer, NoticeContent } from '../common/notice.js';
import { noticeCountdown, noticeTitle } from '../common/noticeText.js';

/** The notice window's preload API. */
export interface NoticeBridge {
	onShow(listener: (content: NoticeContent) => void): void;
	onMaterial(listener: (material: WindowBackgroundMaterial) => void): void;
	respond(answer: NoticeAnswer): void;
}

/** Spoken again at these seconds left, so a screen reader hears the countdown without every tick. */
const announcedSeconds = new Set([30, 20, 10, 5]);

/**
 * Tells the person at this computer that a paired computer asked it to
 * restart or shut down, counting down to the moment it runs. Cancel is
 * focused, so Return and Escape both keep the computer running.
 */
export class NoticePage extends Disposable {
	private answered = false;

	constructor(
		private readonly container: HTMLElement,
		private readonly bridge: NoticeBridge,
		private readonly now: () => number = Date.now,
	) {
		super();
		// The window's backdrop shows through only where the page is transparent, as in the workbench.
		bridge.onMaterial(material => {
			container.ownerDocument.documentElement.dataset.windowMaterial = material;
		});
		bridge.onShow(content => {
			this.render(content);
		});
	}

	private render(content: NoticeContent): void {
		const dialog = append(this.container, $('.notice', { role: 'alertdialog', 'aria-labelledby': 'notice-title', 'aria-describedby': 'notice-detail' }));
		append(append(dialog, $('.notice-icon', { 'aria-hidden': 'true' })), renderIcon(Icons.warning));
		const text = append(dialog, $('.notice-text'));
		append(text, $('h1#notice-title')).textContent = noticeTitle(content);
		const detail = append(text, $('p#notice-detail'));
		const spoken = append(text, $('p.notice-spoken', { 'aria-live': 'polite' }));
		const buttons = append(dialog, $('.notice-buttons'));
		const now = this._register(new Button(buttons, { ...defaultButtonStyles, secondary: true, title: false }));
		now.label = content.action === 'restart' ? nls.localize('notice.restartNow', 'Restart Now') : nls.localize('notice.shutDownNow', 'Shut Down Now');
		const cancel = this._register(new Button(buttons, { ...defaultButtonStyles, title: false }));
		cancel.label = nls.localize('notice.cancel', 'Cancel');
		this._register(cancel.onDidClick(() => {
			this.answer('cancel');
		}));
		this._register(now.onDidClick(() => {
			this.answer('now');
		}));
		this._register(addDisposableListener(this.container.ownerDocument, EventType.KEY_DOWN, event => {
			if (event.key === 'Escape') {
				event.preventDefault();
				this.answer('cancel');
			}
		}));
		const tick = (): void => {
			const line = noticeCountdown(content, this.now());
			if (detail.textContent !== line) {
				detail.textContent = line;
				const seconds = Math.ceil((content.deadline - this.now()) / 1000);
				if (announcedSeconds.has(seconds)) {
					spoken.textContent = line;
				}
			}
		};
		tick();
		const timer = setInterval(tick, 250);
		this._register(toDisposable(() => {
			clearInterval(timer);
		}));
		cancel.focus();
	}

	private answer(answer: NoticeAnswer): void {
		if (!this.answered) {
			this.answered = true;
			this.bridge.respond(answer);
		}
	}
}
