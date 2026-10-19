import { app, BrowserWindow, nativeTheme, screen } from 'electron';
import { join } from 'node:path';
import { isMacintosh } from '../../../base/common/platform.js';
import { isNoticeAnswer, NOTICE_RESPOND_CHANNEL, NOTICE_SHOW_CHANNEL, NOTICE_URL, type NoticeContent } from '../common/notice.js';
import type { NoticePrompt } from './systemActionNotice.js';

const width = 460;

const height = 168;

/** Cancel is the first answer, as in `SystemActionNotices`. */
const answers = { cancel: 0, now: 1 } as const;

/**
 * Shows the notice in a small window of its own, in front of everything and
 * on every desktop, on the screen with the pointer. It answers only to its
 * own page, and closes when its signal aborts.
 */
export const windowPrompt: NoticePrompt = (content: NoticeContent, signal: AbortSignal) => new Promise<number | undefined>(resolve => {
	if (signal.aborted) {
		resolve(undefined);
		return;
	}
	const area = screen.getDisplayNearestPoint(screen.getCursorScreenPoint()).workArea;
	const window = new BrowserWindow({
		width,
		height,
		x: Math.round(area.x + (area.width - width) / 2),
		y: Math.round(area.y + (area.height - height) / 3),
		show: false,
		frame: false,
		resizable: false,
		minimizable: false,
		maximizable: false,
		fullscreenable: false,
		// Only an answer closes it; a stray Command-W must not dismiss the countdown.
		closable: false,
		alwaysOnTop: true,
		title: 'VniLink',
		backgroundColor: nativeTheme.shouldUseDarkColors ? '#2c2d31' : '#ffffff',
		webPreferences: {
			preload: join(__dirname, 'noticePreload.cjs'),
			sandbox: true,
			contextIsolation: true,
			nodeIntegration: false,
			webviewTag: false,
			spellcheck: false,
		},
	});
	let settled = false;
	const finish = (answer: number | undefined): void => {
		if (settled) {
			return;
		}
		settled = true;
		signal.removeEventListener('abort', abort);
		if (!window.isDestroyed()) {
			window.destroy();
		}
		resolve(answer);
	};
	const abort = (): void => {
		finish(undefined);
	};
	signal.addEventListener('abort', abort);
	window.setAlwaysOnTop(true, 'floating');
	window.setVisibleOnAllWorkspaces(true, { visibleOnFullScreen: true });
	window.removeMenu();
	const contents = window.webContents;
	contents.setWindowOpenHandler(() => ({ action: 'deny' }));
	contents.on('will-navigate', event => {
		event.preventDefault();
	});
	contents.ipc.on(NOTICE_RESPOND_CHANNEL, (event, answer: unknown) => {
		// Only this window's own page answers.
		if (event.senderFrame !== contents.mainFrame || event.senderFrame.url !== NOTICE_URL || !isNoticeAnswer(answer)) {
			return;
		}
		finish(answers[answer]);
	});
	contents.once('did-finish-load', () => {
		contents.send(NOTICE_SHOW_CHANNEL, content);
		if (isMacintosh) {
			app.focus({ steal: true });
		}
		window.show();
		window.focus();
	});
	window.once('closed', () => {
		finish(undefined);
	});
	void window.loadURL(NOTICE_URL).catch(() => {
		finish(undefined);
	});
});
