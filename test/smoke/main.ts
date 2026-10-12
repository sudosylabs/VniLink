import { isEngineReply, record } from '../../src/platform/engine/common/engine.js';
import type { BrowserWindow } from 'electron';
import { app, net } from 'electron';
import { mkdtempSync, rmSync } from 'node:fs';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { isAppInfo } from '../../src/platform/ipc/common/desktopApi.js';
import { startApplication } from '../../src/app/electron-main/application.js';
const profile = mkdtempSync(join(tmpdir(), 'vnilink-electron-smoke-'));
app.setPath('userData', profile);
const timeout = setTimeout(() => {
	console.error('Smoke test timed out'); app.exit(1);
}, 40_000);
app.on('quit', () => {
	clearTimeout(timeout); rmSync(profile, { recursive: true, force: true });
});
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
		|| !('api' in result) || JSON.stringify(result.api) !== JSON.stringify(['engineCommand', 'getAppInfo', 'windowAction'])) {
		throw new Error(`Unexpected renderer state: ${JSON.stringify(result)}`);
	}
	const engine: unknown = await window.webContents.executeJavaScript(`(async () => {
    const snapshot = await window.vnilink.engineCommand({ type: 'snapshot' });
    const invitation = await window.vnilink.engineCommand({ type: 'createInvitation', ttlSeconds: 60 });
    let invalidRejected = false;
    try { await window.vnilink.engineCommand({ type: 'shutdown' }); } catch { invalidRejected = true; }
    return { snapshot, invitation, invalidRejected };
  })()`);
	if (!record(engine) || !isEngineReply(engine.snapshot) || engine.snapshot.type !== 'snapshot'
		|| engine.snapshot.snapshot.endpoint_id.length !== 64 || engine.snapshot.snapshot.hosting
		|| !isEngineReply(engine.invitation) || engine.invitation.type !== 'invitation' || !engine.invitation.token
		|| engine.invalidRejected !== true) {
		throw new Error('Native control bridge failed');
	}
	let rejected = false;
	try {
		await window.webContents.executeJavaScript(`window.vnilink.windowAction('invalid')`);
	} catch {
		rejected = true;
	}
	if (!rejected) {
		throw new Error('Invalid window action accepted');
	}
	if ((await net.fetch('vnilink://app/main.cjs')).status !== 404) {
		throw new Error('Unlisted asset accessible');
	}
	console.log('PASS: application loaded, typed bridge works, renderer isolated, invalid action rejected, native Rust state and invitations work, privileged shutdown rejected.');
	app.quit();
}
void startApplication({ showWindow: false, temporaryEngine: true, onReady: verify });
