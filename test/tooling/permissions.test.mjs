import assert from 'node:assert/strict';
import test from 'node:test';
import { build } from 'esbuild';

const result = await build({ stdin: { contents: `export { MachinePermissions } from './src/platform/permissions/electron-main/machinePermissions.js'; export { isPermissionCommand, isPermissionSnapshot } from './src/platform/permissions/common/permissions.js';`, loader: 'ts', resolveDir: process.cwd() }, bundle: true, write: false, format: 'esm', platform: 'node', tsconfig: 'tsconfig.json' });

const { MachinePermissions, isPermissionCommand, isPermissionSnapshot } = await import(`data:text/javascript;base64,${Buffer.from(result.outputFiles[0].text).toString('base64')}`);

function fixture(platform = 'darwin') {
	const states = { screen: 'denied', camera: 'not-determined', microphone: 'granted' };
	const prompts = [];
	const urls = [];
	let trusted = false;
	const preferences = {
		getMediaAccessStatus: kind => states[kind],
		isTrustedAccessibilityClient: prompt => {
			if (prompt) {
				prompts.push('accessibility');
			}
			return trusted;
		},
		askForMediaAccess: async kind => {
			prompts.push(kind);
			states[kind] = 'granted';
			return true;
		},
	};
	return {
		states, prompts, urls, preferences, trust: () => {
			trusted = true;
		}, service: new MachinePermissions(platform, true, preferences, async url => {
			urls.push(url);
		})
	};
}

test('OS statuses are live and passive queries never request consent', async () => {
	const f = fixture();
	let snapshot = await f.service.execute({ type: 'status' });
	assert(isPermissionSnapshot(snapshot));
	assert.deepEqual(snapshot.permissions.map(p => p.status), ['denied', 'denied', 'not-determined', 'granted']);
	assert.deepEqual(f.prompts, []);
	f.states.screen = 'granted';
	f.trust();
	snapshot = f.service.snapshot();
	assert.equal(snapshot.permissions[0].status, 'granted');
	assert.equal(snapshot.permissions[1].status, 'granted');
	assert.equal(snapshot.permissions[1].canRequest, false);
});

test('explicit media request reads back OS consent and denied media uses fixed settings links', async () => {
	const f = fixture();
	const snapshot = await f.service.execute({ type: 'request', kind: 'camera' });
	assert.deepEqual(f.prompts, ['camera']);
	assert.equal(snapshot.permissions[2].status, 'granted');
	f.states.microphone = 'denied';
	await assert.rejects(f.service.execute({ type: 'request', kind: 'microphone' }));
	await f.service.execute({ type: 'openSettings', kind: 'microphone' });
	await f.service.execute({ type: 'openSettings', kind: 'screen' });
	assert.deepEqual(f.urls, ['x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone', 'x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture']);
	assert.deepEqual(f.prompts, ['camera']);
});

test('Accessibility requests do not assume a prompt grants access', async () => {
	const f = fixture();
	const snapshot = await f.service.execute({ type: 'request', kind: 'accessibility' });
	assert.equal(snapshot.permissions[1].status, 'denied');
	assert.deepEqual(f.prompts, ['accessibility']);
});

test('platform limitations never become fake grants or invoke unsupported APIs', async () => {
	const win = fixture('win32');
	const snapshot = win.service.snapshot();
	// Windows has no screen or input consent: the app may already do both.
	assert.equal(snapshot.permissions[0].status, 'not-required');
	assert.equal(snapshot.permissions[1].status, 'not-required');
	assert.equal(snapshot.permissions[0].canRequest || snapshot.permissions[0].canOpenSettings, false);
	assert.equal(snapshot.permissions[2].canRequest, false);
	await win.service.execute({ type: 'openSettings', kind: 'camera' });
	assert.deepEqual(win.urls, ['ms-settings:privacy-webcam']);
	await assert.rejects(win.service.execute({ type: 'request', kind: 'camera' }));
	const linux = fixture('linux');
	assert(linux.service.snapshot().permissions.every(p => p.status === 'unsupported' && !p.canRequest && !p.canOpenSettings));
	await assert.rejects(linux.service.execute({ type: 'openSettings', kind: 'screen' }));
	assert.deepEqual(linux.prompts, []);
});

test('permission bridge rejects arbitrary destinations and malformed snapshots', () => {
	assert(isPermissionCommand({ type: 'status' }));
	assert(isPermissionCommand({ type: 'request', kind: 'camera' }));
	assert(!isPermissionCommand({ type: 'openSettings', kind: 'camera', url: 'https://example.com' }));
	assert(!isPermissionCommand({ type: 'request', kind: 'files' }));
	const f = fixture();
	assert(!isPermissionSnapshot({ ...f.service.snapshot(), extra: true }));
	assert(!isPermissionSnapshot({ development: false, permissions: [] }));
});
