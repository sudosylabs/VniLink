/**
 * Preload of the restart notice window. It exposes two calls on the
 * notice's own channels and nothing of the workbench's IPC.
 */
import { contextBridge, ipcRenderer } from 'electron';
import { isNoticeAnswer, isNoticeContent, NOTICE_RESPOND_CHANNEL, NOTICE_SHOW_CHANNEL, type NoticeContent } from './platform/systemActions/common/notice.js';

if (!process.sandboxed || !process.contextIsolated) {
	throw new Error('Renderer isolation required');
}

let shown = false;

contextBridge.exposeInMainWorld('vnilinkNotice', {
	/** Receives what to show, once. */
	onShow(listener: (content: NoticeContent) => void): void {
		ipcRenderer.on(NOTICE_SHOW_CHANNEL, (_event, content: unknown) => {
			if (!shown && isNoticeContent(content)) {
				shown = true;
				listener(content);
			}
		});
	},
	respond(answer: unknown): void {
		if (isNoticeAnswer(answer)) {
			ipcRenderer.send(NOTICE_RESPOND_CHANNEL, answer);
		}
	},
});
