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

test('hosting starts with the engine, follows status events and survives an engine that becomes available again', async () => {
	let hosting = true;
	let incoming = null;
	const fake = engine(command => command.type === 'snapshot' ? { type: 'snapshot', snapshot: { hosting, incoming } } : { type: 'done' });
	const service = new HostingMainService(fake, log);
	const statuses = [];
	service.onDidChangeStatus(status => statuses.push(status));
	service.start();
	service.start();
	await settle();
	assert.deepEqual(fake.calls, ['startHosting', 'snapshot']);
	assert.deepEqual(service.status, { hosting: true, incoming: null });
	fake.events.fire({ events: [{ type: 'peerRenamed', endpoint_id: 'peer', hostname: null }], resync: false });
	await settle();
	assert.equal(fake.calls.filter(type => type === 'snapshot').length, 1, 'Unrelated events must not re-read the snapshot');
	incoming = { id: '1', generation: '1', role: 'Host', phase: 'Active', peer_id: 'peer' };
	fake.events.fire({ events: [{ type: 'sessionChanged', id: '1', generation: '1', role: 'Host', phase: 'Active', peer_id: 'peer' }], resync: false });
	await settle();
	assert.equal(service.status.incoming.peer_id, 'peer');
	hosting = false;
	fake.events.fire({ events: [{ type: 'hostingChanged', hosting: false }], resync: false });
	await settle();
	assert.equal(service.status.hosting, false);
	assert.equal(fake.calls.filter(type => type === 'startHosting').length, 1, 'A deliberate stop must not be undone');
	fake.availability.fire(false);
	assert.deepEqual(service.status, { hosting: false, incoming: null });
	hosting = true;
	incoming = null;
	fake.availability.fire(true);
	await settle();
	assert.equal(fake.calls.filter(type => type === 'startHosting').length, 2);
	assert.equal(service.status.hosting, true);
	assert.equal(statuses.length, 5, 'Status fires only when it changes');
	service.dispose();
	assert.equal(fake.events.hasListeners(), false, 'Disposed service kept the engine event stream open');
});

test('a failed start is retried on the next resync, not on every event', async () => {
	let fail = true;
	const fake = engine(command => {
		if (command.type === 'startHosting' && fail) {
			throw new Error('Engine busy');
		}
		return command.type === 'snapshot' ? { type: 'snapshot', snapshot: { hosting: !fail, incoming: null } } : { type: 'done' };
	});
	const service = new HostingMainService(fake, log);
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
	const settings = new SystemSettings(item);
	let changes = 0;
	settings.onDidChange(() => changes++);
	await settings.refresh();
	assert.deepEqual(settings.values, { 'application.openAtLogin': true });
	assert.equal(settings.restricted.includes('application.openAtLogin'), false);
	await settings.update('application.openAtLogin', false);
	assert.deepEqual(item.writes, [false]);
	assert.deepEqual(settings.values, { 'application.openAtLogin': false });
	assert.equal(changes, 2);
	await assert.rejects(settings.update('application.runInBackground', true));
	settings.dispose();
	const development = new SystemSettings(loginItem(false, true));
	await development.refresh();
	assert.deepEqual(development.values, { 'application.openAtLogin': false }, 'An unsupported login item must not report a value it cannot change');
	assert(development.restricted.includes('application.openAtLogin'));
	await assert.rejects(development.update('application.openAtLogin', true));
	development.dispose();
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
