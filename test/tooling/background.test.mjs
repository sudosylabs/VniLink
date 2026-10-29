import assert from 'node:assert/strict';
import { mkdtemp, readFile, rm, stat } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';
import { build } from 'esbuild';

const root = fileURLToPath(new URL('../../', import.meta.url));

const product = JSON.parse(await readFile(join(root, 'product.json'), 'utf8'));

const bundle = await build({
	stdin: {
		contents: `export { HostingMainService } from './src/platform/hosting/electron-main/hostingMainService.js';
			export { ScreenHostingMainService } from './src/platform/hosting/electron-main/screenHostingMainService.js';
			export { SystemSettings, AutostartLoginItem, autostartEntry } from './src/platform/configuration/electron-main/systemSettings.js';
			export { Emitter } from './src/base/common/event.js';`,
		loader: 'ts', resolveDir: root,
	},
	bundle: true, write: false, platform: 'node', format: 'esm', target: 'node22', tsconfig: `${root}tsconfig.json`,
	plugins: [{
		name: 'electron-fixture', setup(builder) {
			builder.onResolve({ filter: /^electron$/ }, () => ({ path: 'electron', namespace: 'fixture' }));
			builder.onLoad({ filter: /.*/, namespace: 'fixture' }, () => ({ contents: 'export const app = {}; export class MessageChannelMain { } export const utilityProcess = {}; export const ipcMain = {}; export const screen = {}; export const shell = {}; export const desktopCapturer = {}; export const systemPreferences = {}; export default { app, ipcMain };', loader: 'js' }));
		}
	}],
});

const { HostingMainService, ScreenHostingMainService, SystemSettings, AutostartLoginItem, autostartEntry, Emitter } = await import(`data:text/javascript;base64,${Buffer.from(bundle.outputFiles[0].text).toString('base64')}`);

const settle = async () => {
	for (let step = 0; step < 20; step++) {
		await new Promise(resolve => setImmediate(resolve));
	}
};

function engine(reply) {
	const events = new Emitter();
	const availability = new Emitter();
	const calls = [];
	return {
		available: true, calls, events, availability,
		onDidReceiveEvents: events.event,
		onDidChangeAvailability: availability.event,
		async execute(command) {
			calls.push(command.type);
			return reply(command);
		},
	};
}

const log = { error() { } };

/** The accept setting as `ConfigurationMainService` presents it. */
function configuration(accepting = true) {
	const changes = new Emitter();
	const values = { 'application.acceptConnections': accepting };
	return {
		changes,
		onDidChangeConfiguration: changes.event,
		getValue: key => values[key],
		async updateValue(key, value) {
			values[key] = value;
			changes.fire({});
		},
	};
}

/** An engine whose hosting, sessions and devices the test changes. */
function hostingEngine(state, fail = () => false) {
	return engine(command => {
		if (fail(command)) {
			throw new Error('Engine busy');
		}
		switch (command.type) {
			case 'startHosting': state.hosting = true; return { type: 'done' };
			case 'stopHosting': state.hosting = false; state.sessions = []; return { type: 'done' };
			case 'stopHostedPeer': state.sessions = state.sessions.filter(session => session.id !== command.sessionId); state.services = (state.services ?? []).filter(service => service.connectionId !== command.sessionId); return { type: 'done' };
			case 'hostedSessions': return { type: 'sessions', sessions: state.sessions };
			case 'hostedServices': return { type: 'services', services: state.services ?? [] };
			case 'snapshot': return { type: 'snapshot', snapshot: { hosting: state.hosting, devices: state.devices } };
		}
		throw new Error(command.type);
	});
}

const session = (id, peer, phase = 'Active') => ({ id, generation: '1', role: 'Host', phase, peer_id: peer });

test('hosting follows the saved accept choice, names connected devices and disconnects one at a time', async () => {
	const state = { hosting: false, sessions: [], devices: [{ endpoint_id: 'a', pet_name: 'Work laptop', hostname: 'host-a' }, { endpoint_id: 'b', pet_name: null, hostname: 'host-b' }] };
	const fake = hostingEngine(state);
	const settings = configuration();
	const service = new HostingMainService(fake, settings, log);
	const statuses = [];
	service.onDidChangeStatus(status => statuses.push(status));
	service.start();
	service.start();
	await settle();
	assert.deepEqual(fake.calls.filter(type => type.endsWith('Hosting')), ['startHosting']);
	assert.deepEqual(service.status, { available: true, accepting: true, hosting: true, sessions: [] });
	// Idle presence links from paired computers are not someone connected.
	state.sessions = [session('1', 'a'), session('2', 'b', 'Negotiating'), session('3', 'c', 'Stopping'), session('4', 'd')];
	fake.events.fire({ events: [{ type: 'sessionChanged', id: '4', generation: '1', role: 'Host', phase: 'Active', peer_id: 'd' }], resync: false });
	await settle();
	assert.deepEqual(service.status.sessions, [], 'Idle links must not count as connected');
	const running = (connectionId, phase = 'Active') => ({ id: `s${connectionId}`, connectionId, generation: '1', peer_id: 'x', kind: 'Desktop', phase });
	state.services = [running('1'), running('2', 'Waiting'), running('3'), running('4', { Closed: { cause: 'Local' } })];
	fake.events.fire({ events: [{ type: 'hostedServiceChanged', service: running('1') }], resync: false });
	await settle();
	assert.deepEqual(service.status.sessions, [{ id: '1', name: 'Work laptop', phase: 'Active' }, { id: '2', name: 'host-b', phase: 'Negotiating' }], 'Only sessions carrying a live service count; stopping ones are already leaving');
	const reads = fake.calls.filter(type => type === 'snapshot').length;
	fake.events.fire({ events: [{ type: 'inputFailed', id: '1', message: 'denied' }], resync: false });
	await settle();
	assert.equal(fake.calls.filter(type => type === 'snapshot').length, reads, 'Unrelated events must not re-read the engine');
	await service.disconnect('1');
	assert.deepEqual(service.status.sessions.map(peer => peer.id), ['2']);
	assert.equal(state.hosting, true, 'Disconnecting one device must leave hosting on');
	await service.setAccepting(false);
	await settle();
	assert.equal(state.hosting, false);
	assert.deepEqual(service.status, { available: true, accepting: false, hosting: false, sessions: [] });
	fake.events.fire({ events: [], resync: true });
	await settle();
	assert.equal(state.hosting, false, 'A resync must not undo a pause');
	fake.availability.fire(false);
	assert.equal(service.status.available, false);
	fake.availability.fire(true);
	await settle();
	assert.equal(state.hosting, false, 'An engine that comes back stays paused');
	await service.setAccepting(true);
	await settle();
	assert.equal(state.hosting, true);
	assert(statuses.every((status, index) => index === 0 || JSON.stringify(status) !== JSON.stringify(statuses[index - 1])), 'Status fires only when it changes');
	service.dispose();
	assert.equal(fake.events.hasListeners(), false, 'Disposed service kept the engine event stream open');
	assert.equal(settings.changes.hasListeners(), false, 'Disposed service kept listening to settings');
});

test('a paused computer starts without hosting, and a failed change is retried on the next resync', async () => {
	const paused = { hosting: true, sessions: [], devices: [] };
	const stopped = hostingEngine(paused);
	const first = new HostingMainService(stopped, configuration(false), log);
	first.start();
	await settle();
	assert.deepEqual(stopped.calls.filter(type => type.endsWith('Hosting')), ['stopHosting'], 'A saved pause must hold from the first start');
	first.dispose();
	let fail = true;
	const state = { hosting: false, sessions: [], devices: [] };
	const fake = hostingEngine(state, command => fail && command.type === 'startHosting');
	const service = new HostingMainService(fake, configuration(), log);
	service.start();
	await settle();
	assert.equal(service.status.hosting, false);
	fake.events.fire({ events: [{ type: 'sessionChanged' }], resync: false });
	await settle();
	assert.equal(fake.calls.filter(type => type === 'startHosting').length, 1);
	fail = false;
	fake.events.fire({ events: [], resync: true });
	await settle();
	assert.equal(fake.calls.filter(type => type === 'startHosting').length, 2);
	assert.equal(service.status.hosting, true);
	service.dispose();
});

function loginItem(supported, enabled = false) {
	const writes = [];
	return {
		supported, writes,
		async read() {
			return enabled;
		},
		async write(value) {
			writes.push(value);
			enabled = value;
		},
	};
}

test('start at login shows the operating system state and is restricted where this launch cannot register', async () => {
	const item = loginItem(true, true);
	const settings = new SystemSettings(item, async () => true);
	let changes = 0;
	settings.onDidChange(() => changes++);
	await settings.refresh();
	assert.deepEqual(settings.values, { 'application.openAtLogin': true });
	assert.equal(settings.restricted.includes('application.openAtLogin'), false);
	const before = changes;
	await settings.update('application.openAtLogin', false);
	assert.deepEqual(item.writes, [false]);
	assert.deepEqual(settings.values, { 'application.openAtLogin': false });
	assert.equal(changes, before + 1);
	await assert.rejects(settings.update('application.runInBackground', true));
	settings.dispose();
	const development = new SystemSettings(loginItem(false, true), async () => true);
	await development.refresh();
	assert.deepEqual(development.values, { 'application.openAtLogin': false }, 'An unsupported login item must not report a value it cannot change');
	assert(development.restricted.includes('application.openAtLogin'));
	await assert.rejects(development.update('application.openAtLogin', true));
	development.dispose();
});

test('running in the background is offered only while a tray icon can show', async () => {
	let shown = false;
	const settings = new SystemSettings(loginItem(true), async () => shown);
	await settings.refresh();
	assert(settings.restricted.includes('application.runInBackground'));
	let changes = 0;
	settings.onDidChange(() => changes++);
	shown = true;
	await settings.refresh();
	assert.equal(settings.restricted.includes('application.runInBackground'), false);
	assert.equal(changes, 1, 'A tray host appearing must republish the settings');
	settings.dispose();
});

test('the Linux autostart entry quotes the executable and is removed when disabled', async () => {
	assert.throws(() => autostartEntry('/opt/vni\nlink'));
	assert.match(autostartEntry('/opt/Vni "Link"/$HOME/50%/a\\b'), /^Exec="\/opt\/Vni \\\\"Link\\\\"\/\\\\\$HOME\/50%%\/a\\\\\\\\b" --hidden$/m);
	const directory = await mkdtemp(join(tmpdir(), 'vnilink-autostart-'));
	try {
		const path = join(directory, 'autostart', product.platforms.linux.desktopFileName);
		const item = new AutostartLoginItem(path, '/opt/VniLink/vnilink');
		assert.equal(await item.read(), false);
		await item.write(true);
		assert.equal(await item.read(), true);
		assert.match(await readFile(path, 'utf8'), /^Exec="\/opt\/VniLink\/vnilink" --hidden$/m);
		await item.write(false);
		assert.equal(await item.read(), false);
		await assert.rejects(stat(`${path}.tmp`));
	} finally {
		await rm(directory, { recursive: true, force: true });
	}
});

test('the screen host follows the primary display, macOS consent and engine restarts; Linux follows screen sharing and keeps its portal grant', async () => {
	const consent = new Emitter();
	const availability = new Emitter();
	const calls = [];
	let granted = false;
	let display = { displayId: 1, width: 1440, height: 900, pixelWidth: 2880, pixelHeight: 1800 };
	let displaysChanged = () => undefined;
	const engine = {
		available: true, onDidChangeAvailability: availability.event,
		async execute(command) {
			calls.push(command);
			return { type: 'done' };
		},
	};
	const permissions = { isGranted: kind => kind === 'screen' && granted, onDidChangeConsent: consent.event };
	const settings = new Emitter();
	let sharing = false;
	const configuration = {
		get snapshot() {
			return { values: { 'sharing.screen': sharing } };
		},
		onDidChangeConfiguration: settings.event,
	};
	const environment = (platform) => ({
		platform, primaryDisplay: () => display, onDidChangeDisplays: listener => {
			displaysChanged = listener;
			return () => undefined;
		},
	});
	const types = () => calls.map(command => command.type);
	const mac = new ScreenHostingMainService(engine, permissions, { warn: () => undefined }, configuration, environment('darwin'));
	try {
		await mac.start();
		assert.deepEqual(types(), [], 'macOS shared the screen without Screen Recording');
		granted = true;
		consent.fire();
		await settle();
		assert.deepEqual(calls, [{ type: 'startNativeScreenSharing', ...display }]);
		displaysChanged();
		await settle();
		assert.equal(calls.length, 1, 'An unchanged display restarted the host');
		display = { ...display, displayId: 2 };
		displaysChanged();
		await settle();
		assert.deepEqual(types(), ['startNativeScreenSharing', 'stopNativeScreenSharing', 'startNativeScreenSharing']);
		assert.equal(calls[2].displayId, 2);
		availability.fire(false);
		availability.fire(true);
		await settle();
		assert.deepEqual(types().slice(3), ['startNativeScreenSharing'], 'A restarted engine was left without a host');
	} finally {
		mac.dispose();
	}
	calls.length = 0;
	granted = false;
	const windows = new ScreenHostingMainService(engine, permissions, { warn: () => undefined }, configuration, environment('win32'));
	try {
		await windows.start();
		assert.deepEqual(types(), ['startNativeScreenSharing'], 'Windows waited for a consent it does not have');
	} finally {
		windows.dispose();
	}
	calls.length = 0;
	displaysChanged = undefined;
	const linux = new ScreenHostingMainService(engine, permissions, { warn: () => undefined }, configuration, environment('linux'));
	try {
		await linux.start();
		assert.deepEqual(types(), [], 'Linux showed the portal dialog while screen sharing was off');
		sharing = true;
		settings.fire();
		await settle();
		assert.deepEqual(types(), ['startNativeScreenSharing'], 'Linux did not ask the portal once sharing was on');
		assert.equal(displaysChanged, undefined, 'A display change would restart the host and show the portal dialog again');
		sharing = false;
		settings.fire();
		await settle();
		assert.deepEqual(types(), ['startNativeScreenSharing', 'stopNativeScreenSharing'], 'Turning sharing off kept the portal grant');
	} finally {
		linux.dispose();
	}
	calls.length = 0;
	const other = new ScreenHostingMainService(engine, permissions, { warn: () => undefined }, configuration, environment('other'));
	try {
		await other.start();
		assert.deepEqual(types(), [], 'A platform without a native host was asked to share');
	} finally {
		other.dispose();
	}
});
