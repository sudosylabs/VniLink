import type { BrowserWindow } from 'electron';
import { app, net } from 'electron';
import { mkdtempSync, rmSync } from 'node:fs';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { isAppInfo } from '../../src/platform/ipc/common/desktopApi';
import { startApplication } from '../../src/app/electron-main/application';
const profile = mkdtempSync(join(tmpdir(), 'vnilink-electron-smoke-'));
app.setPath('userData', profile);
const timeout = setTimeout(() => { console.error('Smoke test timed out'); app.exit(1); }, 20_000);
app.on('quit', () => { clearTimeout(timeout); rmSync(profile, { recursive: true, force: true }); });
async function verify(window: BrowserWindow): Promise<void> {
	await window.webContents.executeJavaScript(`new Promise((resolve, reject) => {
    if (document.querySelector('h1')) return resolve(true);
    const timeout = setTimeout(() => { observer.disconnect(); reject(new Error('Workbench did not render')); }, 5000);
    const observer = new MutationObserver(() => {
      if (document.querySelector('h1')) { clearTimeout(timeout); observer.disconnect(); resolve(true); }
    });
    observer.observe(document.body, { childList: true, subtree: true });
  })`);
	const result: unknown = await window.webContents.executeJavaScript(`(async () => {
    const info = await window.vnilink.getAppInfo();
    return { info, nodeExposed: typeof require !== 'undefined' || typeof process !== 'undefined',
      title: document.querySelector('h1')?.textContent, api: Object.keys(window.vnilink).sort() };
  })()`);
	if (typeof result !== 'object' || result === null
		|| !('nodeExposed' in result) || result.nodeExposed !== false
		|| !('info' in result) || !isAppInfo(result.info) || result.info.version !== app.getVersion()
		|| !('title' in result) || result.title !== 'Your computers'
		|| !('api' in result) || JSON.stringify(result.api) !== JSON.stringify(['getAppInfo', 'windowAction'])) {
		throw new Error(`Unexpected renderer state: ${JSON.stringify(result)}`);
	}
	let rejected = false;
	try { await window.webContents.executeJavaScript(`window.vnilink.windowAction('invalid')`); } catch { rejected = true; }
	if (!rejected) { throw new Error('Invalid window action accepted'); }
	if ((await net.fetch('vnilink://app/main.cjs')).status !== 404) { throw new Error('Unlisted asset accessible'); }
	console.log('PASS: application loaded, typed bridge works, renderer isolated, invalid action rejected, Rust availability honest.');
	app.quit();
}
void startApplication({ showWindow: false, onReady: verify });
