import * as nls from '../../../nls.js';
import type { NoticeContent } from './notice.js';

/** Who asked, and for what. */
export function noticeTitle(content: NoticeContent): string {
	return content.action === 'restart'
		? nls.localize('notice.restartTitle', '{0} wants to restart this computer', content.name)
		: nls.localize('notice.shutdownTitle', '{0} wants to shut down this computer', content.name);
}

/** The seconds left to the deadline, rounded up, and what to do meanwhile. */
export function noticeCountdown(content: NoticeContent, now: number): string {
	const seconds = Math.max(0, Math.ceil((content.deadline - now) / 1000));
	return content.action === 'restart'
		? nls.localize('notice.restartIn', 'Restarting in {0} s. Save your work, or cancel.', seconds)
		: nls.localize('notice.shutdownIn', 'Shutting down in {0} s. Save your work, or cancel.', seconds);
}
