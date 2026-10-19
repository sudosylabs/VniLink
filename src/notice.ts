/** Entry of the restart notice window; see `platform/systemActions/browser/noticePage.ts`. */
import { mainWindow } from './base/browser/window.js';
import { NoticePage, type NoticeBridge } from './platform/systemActions/browser/noticePage.js';

const bridge: unknown = Reflect.get(mainWindow, 'vnilinkNotice');

if (typeof bridge === 'object' && bridge !== null) {
	new NoticePage(mainWindow.document.body, bridge as NoticeBridge);
}
