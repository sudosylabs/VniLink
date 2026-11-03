// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import * as nls from '../../../nls.js';
import type { NoticeContent } from './notice.js';

/** Who asked, and for what. */
export function noticeTitle(content: NoticeContent): string {
	switch (content.action) {
		case 'restart': return nls.localize('notice.restartTitle', '{0} wants to restart this computer', content.name);
		case 'shutdown': return nls.localize('notice.shutdownTitle', '{0} wants to shut down this computer', content.name);
		case 'admit': return nls.localize('notice.admitTitle', '{0} wants to use this computer', content.name);
	}
}

/** The seconds left to the deadline, rounded up, and what to do meanwhile. */
export function noticeCountdown(content: NoticeContent, now: number): string {
	const seconds = Math.max(0, Math.ceil((content.deadline - now) / 1000));
	switch (content.action) {
		case 'restart': return nls.localize('notice.restartIn', 'Restarting in {0} s. Save your work, or cancel.', seconds);
		case 'shutdown': return nls.localize('notice.shutdownIn', 'Shutting down in {0} s. Save your work, or cancel.', seconds);
		case 'admit': return nls.localize('notice.admitIn', 'It can use what you share once you allow it. Declining in {0} s.', seconds);
	}
}

/** The button that does what was asked at once. */
export function noticeNowLabel(content: NoticeContent): string {
	switch (content.action) {
		case 'restart': return nls.localize('notice.restartNow', 'Restart Now');
		case 'shutdown': return nls.localize('notice.shutDownNow', 'Shut Down Now');
		case 'admit': return nls.localize('notice.allow', 'Allow');
	}
}

/** The button that leaves the computer as it is, the default. */
export function noticeCancelLabel(content: NoticeContent): string {
	return content.action === 'admit' ? nls.localize('notice.decline', 'Decline') : nls.localize('notice.cancel', 'Cancel');
}
