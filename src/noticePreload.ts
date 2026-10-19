/**
 * Preload of the restart notice window. It exposes three calls on the
 * notice's own channels and nothing of the workbench's IPC.
 */
import { contextBridge, ipcRenderer } from 'electron';
import { isNoticeAnswer, isNoticeContent, NOTICE_MATERIAL_CHANNEL, NOTICE_RESPOND_CHANNEL, NOTICE_SHOW_CHANNEL, type NoticeContent } from './platform/systemActions/common/notice.js';
import { isWindowBackgroundMaterial, type WindowBackgroundMaterial } from './platform/window/common/windowMaterial.js';

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
	/** Receives the window's backdrop, now and on every change. */
	onMaterial(listener: (material: WindowBackgroundMaterial) => void): void {
		ipcRenderer.on(NOTICE_MATERIAL_CHANNEL, (_event, material: unknown) => {
			if (isWindowBackgroundMaterial(material)) {
				listener(material);
			}
		});
	},
	respond(answer: unknown): void {
		if (isNoticeAnswer(answer)) {
			ipcRenderer.send(NOTICE_RESPOND_CHANNEL, answer);
		}
	},
});
