import assert from 'node:assert/strict';
import { mkdtemp, readFile, rm, stat } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';
import { build } from 'esbuild';

const root = fileURLToPath(new URL('../../', import.meta.url));

const bundle = await build({
	stdin: {
		contents: `export { HostingMainService } from './src/platform/hosting/electron-main/hostingMainService.js';
			export { SystemSettings, AutostartLoginItem, autostartEntry } from './src/platform/configuration/electron-main/systemSettings.js';
			export { Emitter } from './src/base/common/event.js';`,
		loader: 'ts', resolveDir: root,
	},
	bundle: true, write: false, platform: 'node', format: 'esm', target: 'node22', tsconfig: `${root}tsconfig.json`,
	plugins: [{
		name: 'electron-fixture', setup(builder) {
			builder.onResolve({ filter: /^electron$/ }, () => ({ path: 'electron', namespace: 'fixture' }));
			builder.onLoad({ filter: /.*/, namespace: 'fixture' }, () => ({ contents: 'export const app = {}; export class MessageChannelMain { } export const utilityProcess = {}; export const ipcMain = {}; export default { app, ipcMain };', loader: 'js' }));
		}
	}],
});

const { HostingMainService, SystemSettings, AutostartLoginItem, autostartEntry, Emitter } = await import(`data:text/javascript;base64,${Buffer.from(bundle.outputFiles[0].text).toString('base64')}`);

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
			case 'stopHostedPeer': state.sessions = state.sessions.filter(session => session.id !== command.sessionId); return { type: 'done' };
			case 'hostedSessions': return { type: 'sessions', sessions: state.sessions };
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
	state.sessions = [session('1', 'a'), session('2', 'b', 'Negotiating'), session('3', 'c', 'Stopping')];
	fake.events.fire({ events: [{ type: 'sessionChanged', id: '1', generation: '1', role: 'Host', phase: 'Active', peer_id: 'a' }], resync: false });
	await settle();
	assert.deepEqual(service.status.sessions, [{ id: '1', name: 'Work laptop', phase: 'Active' }, { id: '2', name: 'host-b', phase: 'Negotiating' }], 'Stopping sessions are already leaving');
	const reads = fake.calls.filter(type => type === 'snapshot').length;
	fake.events.fire({ events: [{ type: 'keyframeRequested', id: '1' }], resync: false });
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
		const path = join(directory, 'autostart', 'fr.sudosy.vnilink.desktop');
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
