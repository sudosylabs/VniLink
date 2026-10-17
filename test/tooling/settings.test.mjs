import assert from 'node:assert/strict';
import test from 'node:test';
import { fakeEngine, fakeMainProcess } from './fixtures.mjs';
import { build } from 'esbuild';
import { mkdtemp, mkdir, readFile, writeFile, readdir, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

const output = await build({ stdin: { contents: `export { ConfigurationStore } from './src/platform/configuration/node/configurationStore.js'; export { ConfigurationSnapshotService } from './src/platform/configuration/electron-browser/configurationSnapshotService.js'; export { defaultConfiguration, isConfigurationUpdate, isConfigurationSnapshot } from './src/platform/configuration/common/configurationSnapshot.js'; export { ConnectionService } from './src/workbench/services/connection/connectionServiceImpl.js'; export { ConnectionReadbackError } from './src/workbench/services/connection/connectionService.js'; export { validateRelays } from './src/workbench/services/connection/relayValidation.js';`, resolveDir: process.cwd(), loader: 'ts' }, bundle: true, write: false, platform: 'node', format: 'esm', tsconfig: 'tsconfig.json' });

const { ConfigurationStore, ConfigurationSnapshotService, defaultConfiguration, isConfigurationUpdate, isConfigurationSnapshot, ConnectionService, ConnectionReadbackError, validateRelays } = await import(`data:text/javascript;base64,${Buffer.from(output.outputFiles[0].text).toString('base64')}`);

test('local preferences survive reopen and concurrent writes preserve unrelated keys', async () => {
	const dir = await mkdtemp(join(tmpdir(), 'vnilink-settings-'));
	const path = join(dir, 'settings.json');
	const store = new ConfigurationStore(path);
	let notifications = 0;
	store.onDidChangeConfiguration(() => notifications++);
	try {
		await store.initialize();
		assert.deepEqual(store.snapshot.values, defaultConfiguration);
		await Promise.all([store.updateValue({ key: 'appearance.theme', value: 'dark' }), store.updateValue({ key: 'appearance.nativeBackground', value: false })]);
		assert.equal(notifications, 2);
		assert.equal(store.snapshot.revision, 2);
		const contents = JSON.parse(await readFile(path, 'utf8'));
		assert.deepEqual(contents, { ...defaultConfiguration, 'appearance.theme': 'dark', 'appearance.nativeBackground': false });
		assert.deepEqual(await readdir(dir), ['settings.json']);
		const reopened = new ConfigurationStore(path);
		try {
			await reopened.initialize();
			assert.deepEqual(reopened.snapshot.values, contents);
		} finally {
			reopened.dispose();
		}
		await assert.rejects(store.updateValue({ key: 'unknown', value: true }), /Invalid/);
		assert.equal(notifications, 2);
	} finally {
		store.dispose();
		await rm(dir, { recursive: true, force: true });
	}
});

test('invalid settings are preserved and blocked from being overwritten', async () => {
	const dir = await mkdtemp(join(tmpdir(), 'vnilink-settings-'));
	const path = join(dir, 'settings.json');
	for (const contents of ['{broken', '{"appearance.theme":"unknown"}', '{"future.setting":true}', '[]']) {
		await writeFile(path, contents);
		const store = new ConfigurationStore(path);
		try {
			await store.initialize();
			assert.ok(store.snapshot.error);
			assert.deepEqual(store.snapshot.values, defaultConfiguration);
			await assert.rejects(store.updateValue({ key: 'appearance.theme', value: 'light' }));
			assert.equal(await readFile(path, 'utf8'), contents);
		} finally {
			store.dispose();
		}
	}
	await rm(dir, { recursive: true, force: true });
});

test('failed atomic writes keep effective values and later writes recover', async () => {
	const dir = await mkdtemp(join(tmpdir(), 'vnilink-settings-'));
	const path = join(dir, 'settings.json');
	const store = new ConfigurationStore(path);
	try {
		await store.initialize();
		await mkdir(path);
		let events = 0;
		store.onDidChangeConfiguration(() => events++);
		await assert.rejects(store.updateValue({ key: 'appearance.theme', value: 'dark' }));
		assert.equal(events, 0);
		assert.deepEqual(store.snapshot.values, defaultConfiguration);
		assert.deepEqual(await readdir(dir), ['settings.json']);
		await rm(path, { recursive: true });
		await store.updateValue({ key: 'appearance.theme', value: 'light' });
		assert.equal(events, 1);
		assert.equal(store.snapshot.values['appearance.theme'], 'light');
	} finally {
		store.dispose();
		await rm(dir, { recursive: true, force: true });
	}
});

test('renderer configuration rejects stale initialization and releases its subscription', async () => {
	let finish;
	let receive;
	let unsubscribed = 0;
	const service = new ConfigurationSnapshotService(fakeMainProcess({
		configuration: {
			call(command) {
				return command === 'getSnapshot' ? new Promise(resolve => {
					finish = resolve;
				}) : Promise.reject(new Error('disk failure'));
			},
			listen(event) {
				assert.equal(event, 'onDidChangeConfiguration');
				return listener => {
					receive = listener;
					return { dispose: () => unsubscribed++ };
				};
			},
		},
	}));
	const loading = service.initialize();
	const incomingQuality = { ...defaultConfiguration['desktop.quality'] };
	receive({ revision: 2, values: { ...defaultConfiguration, 'appearance.theme': 'dark', 'desktop.quality': incomingQuality }, error: null });
	incomingQuality.max_frame_rate = 10;
	assert.equal(service.snapshot.values['desktop.quality'].max_frame_rate, 30);
	finish({ revision: 0, values: defaultConfiguration, error: null });
	await loading;
	assert.equal(service.snapshot.values['appearance.theme'], 'dark');
	await assert.rejects(service.updateValue({ key: 'appearance.theme', value: 'light' }));
	assert.equal(service.snapshot.values['appearance.theme'], 'dark');
	service.dispose();
	assert.equal(unsubscribed, 1);
	receive({ revision: 3, values: defaultConfiguration, error: null });
	assert.equal(service.snapshot.revision, 2);
	assert.equal(isConfigurationUpdate({ key: 'appearance.theme', value: 'invalid' }), false);
	assert.equal(isConfigurationUpdate({ key: 'appearance.nativeBackground', value: true, extra: true }), false);
	assert.equal(isConfigurationSnapshot({ revision: -1, values: defaultConfiguration, error: null }), false);
});

function relay(url, authenticated = false, replacementToken = '') {
	return { url, authenticated, replacementToken };
}

test('relay validation follows count, HTTPS, loopback, duplicate and credential rules', () => {
	const saved = new Set(['https://saved.example/']);
	for (const value of ['https://relay.example/', 'http://localhost:3340/', 'http://127.0.0.2/', 'http://[::1]/']) {
		assert.deepEqual(validateRelays([relay(value)], saved), []);
	}
	for (const value of ['http://remote.example/', 'https://name:secret@relay.example/', 'https://relay.example/?', 'https://relay.example/#', 'https://relay.example:0/', ' https://relay.example/', 'https://relay.example/with space']) {
		assert.equal(validateRelays([relay(value)], saved)[0].field, 'url');
	}
	assert.equal(validateRelays([], saved).length, 1);
	assert.equal(validateRelays(Array.from({ length: 9 }, (_, i) => relay(`https://relay${i}.example/`)), saved).length, 1);
	assert.equal(validateRelays([relay('https://relay.example'), relay('https://relay.example/')], saved).length, 1);
	assert.deepEqual(validateRelays([relay('https://saved.example/', true)], saved), []);
	assert.equal(validateRelays([relay('https://changed.example/', true)], saved)[0].field, 'token');
	assert.equal(validateRelays([relay('https://saved.example/', true, 'has space')], saved)[0].field, 'token');
	assert.equal(validateRelays([relay('https://saved.example/', true, 'é'.repeat(129))], saved)[0].field, 'token');
});

function connectionFixture() {
	let saved = [];
	const commands = [];
	const api = {
		async engineCommand(command) {
			commands.push(command);
			if (command.type === 'savedRelays') {
				return { type: 'relays', relays: saved };
			}
			if (command.type === 'relayRestartRequired') {
				return { type: 'restartRequired', required: saved.length > 0 };
			}
			if (command.type === 'snapshot') {
				return { type: 'snapshot', snapshot: { public_relay: true, relay_urls: [] } };
			}
			if (command.type === 'saveCustomRelays') {
				saved = command.relays.map(({ url, authenticated }) => ({ url, authenticated }));
			}
			if (command.type === 'setRelayPublic') {
				saved = [];
			}
			return { type: 'done' };
		}
	};
	return { api, commands };
}

test('relay saves read back Rust state and preserve active configuration and saved credentials', async () => {
	const { api, commands } = connectionFixture();
	const service = new ConnectionService(fakeEngine(api, true));
	const saved = await service.save({ mode: 'custom', relays: [relay('https://relay.example/', true, 'opaque-token')] });
	assert.equal(saved.saved[0].authenticated, true);
	assert.equal(saved.active.publicRelay, true);
	assert.ok(!JSON.stringify(saved).includes('opaque-token'));
	await service.save({ mode: 'custom', relays: [relay('https://relay.example/', true)] });
	assert.equal(commands.filter(command => command.type === 'saveCustomRelays')[1].relays[0].replacementToken, '');
	assert.deepEqual((await service.save({ mode: 'public' })).saved, []);
});

test('invalid relay drafts never reach a write and unavailable engines never receive a request', async () => {
	const { api, commands } = connectionFixture();
	const service = new ConnectionService(fakeEngine(api, true));
	await assert.rejects(service.save({ mode: 'custom', relays: [relay('http://remote.example/')] }));
	assert.ok(!commands.some(command => command.type === 'saveCustomRelays'));
	commands.length = 0;
	await assert.rejects(new ConnectionService(fakeEngine(api, false)).load());
	assert.equal(commands.length, 0);
});

test('a committed relay save with failed read-back is distinguished from a write failure', async () => {
	const { api } = connectionFixture();
	const original = api.engineCommand;
	let committed = false;
	api.engineCommand = async command => {
		if (committed) {
			throw new Error('read-back failed');
		}
		const reply = await original(command);
		if (command.type === 'setRelayPublic') {
			committed = true;
		}
		return reply;
	};
	await assert.rejects(new ConnectionService(fakeEngine(api, true)).save({ mode: 'public' }), ConnectionReadbackError);
});

test('queued relay saves capture their drafts and recover after a write failure', async () => {
	const { api, commands } = connectionFixture();
	const original = api.engineCommand;
	let failures = 1;
	api.engineCommand = async command => {
		if (command.type === 'saveCustomRelays' && failures-- > 0) {
			throw new Error('write failure');
		}
		return await original(command);
	};
	const service = new ConnectionService(fakeEngine(api, true));
	const draft = [relay('https://second.example/')];
	const first = service.save({ mode: 'custom', relays: [relay('https://first.example/')] });
	const second = service.save({ mode: 'custom', relays: draft });
	draft[0].url = 'https://mutated.example/';
	await assert.rejects(first, /write failure/);
	assert.equal((await second).saved[0].url, 'https://second.example/');
	assert.equal(commands.filter(command => command.type === 'saveCustomRelays').length, 1);
});

test('desktop defaults persist, validate, and capture mutable drafts before writes', async () => {
	const dir = await mkdtemp(join(tmpdir(), 'vnilink-desktop-defaults-'));
	const path = join(dir, 'settings.json');
	const store = new ConfigurationStore(path);
	try {
		await writeFile(path, JSON.stringify({ 'appearance.theme': 'dark', 'appearance.nativeBackground': false }));
		await store.initialize();
		assert.equal(store.snapshot.error, null);
		const quality = { max_bitrate_kbps: 5000, max_frame_rate: 60, max_width: 1920, max_height: 1080 };
		const saving = store.updateValue({ key: 'desktop.quality', value: quality });
		quality.max_frame_rate = 10;
		await saving;
		const reopened = new ConfigurationStore(path);
		try {
			await reopened.initialize();
			assert.equal(reopened.snapshot.values['desktop.quality'].max_frame_rate, 60);
			assert.equal(reopened.snapshot.values['appearance.theme'], 'dark');
		} finally {
			reopened.dispose();
		}
		assert(!isConfigurationUpdate({ key: 'desktop.quality', value: { ...quality, max_frame_rate: 61 } }));
	} finally {
		store.dispose();
		await rm(dir, { recursive: true, force: true });
	}
});

test('input defaults and shared folders persist across restarts without retaining mutable caller objects', async () => {
	const dir = await mkdtemp(join(tmpdir(), 'vnilink-input-defaults-'));
	const path = join(dir, 'settings.json');
	try {
		const store = new ConfigurationStore(path);
		await store.initialize();
		const defaults = { mode: 'Map', swap: true, pointer: { sensitivity_percent: 175, reverse_wheel: true, swap_buttons: true } };
		const saving = store.updateValue({ key: 'input.defaults', value: defaults });
		defaults.pointer.sensitivity_percent = 200;
		await saving;
		await store.updateValue({ key: 'files.roots', value: [dir] });
		const reopened = new ConfigurationStore(path);
		await reopened.initialize();
		assert.equal(reopened.snapshot.values['input.defaults'].pointer.sensitivity_percent, 175);
		assert.deepEqual(reopened.snapshot.values['files.roots'], [dir]);
		assert(Object.isFrozen(reopened.snapshot.values['input.defaults'].pointer));
		assert(Object.isFrozen(reopened.snapshot.values['files.roots']));
		await assert.rejects(store.updateValue({ key: 'input.defaults', value: { ...defaults, pointer: { ...defaults.pointer, sensitivity_percent: 401 } } }));
		await assert.rejects(store.updateValue({ key: 'files.roots', value: ['relative/path'] }));
		store.dispose();
		reopened.dispose();
	} finally {
		await rm(dir, { recursive: true, force: true });
	}
});

test('codec choice persists and old settings migrate to Auto', async () => {
	const dir = await mkdtemp(join(tmpdir(), 'vnilink-codec-settings-'));
	const path = join(dir, 'settings.json');
	try {
		const oldSettings = Object.fromEntries(Object.entries(defaultConfiguration).filter(([key]) => key !== 'desktop.codec'));
		await writeFile(path, JSON.stringify(oldSettings));
		const store = new ConfigurationStore(path);
		await store.initialize();
		assert.equal(store.snapshot.error, null);
		assert.equal(store.snapshot.values['desktop.codec'], 'auto');
		await store.updateValue({ key: 'desktop.codec', value: 'vp9' });
		store.dispose();
		const reopened = new ConfigurationStore(path);
		await reopened.initialize();
		assert.equal(reopened.snapshot.values['desktop.codec'], 'vp9');
		await assert.rejects(reopened.updateValue({ key: 'desktop.codec', value: 'bogus' }));
		reopened.dispose();
	} finally {
		await rm(dir, { recursive: true, force: true });
	}
});

test('media sources and recording defaults persist, reject invalid limits and preserve older configuration', async () => {
	const dir = await mkdtemp(join(tmpdir(), 'vnilink-media-settings-'));
	const path = join(dir, 'settings.json');
	const store = new ConfigurationStore(path);
	try {
		const previous = { ...defaultConfiguration };
		delete previous['media.sources'];
		delete previous['recording.defaults'];
		await writeFile(path, JSON.stringify(previous));
		await store.initialize();
		assert.equal(store.snapshot.error, null);
		const sources = { cameras: ['camera-one'], microphones: ['mic-one'], system: [] };
		await store.updateValue({ key: 'media.sources', value: sources });
		sources.cameras.push('mutated');
		const limits = { directory: '', maxBytes: '2097152', maxDurationMs: '60000', maxSegments: 2, maxTotalBytes: '4194304' };
		await store.updateValue({ key: 'recording.defaults', value: limits });
		assert(!isConfigurationUpdate({ key: 'recording.defaults', value: { ...limits, maxBytes: '0' } }));
		assert(!isConfigurationUpdate({ key: 'media.sources', value: { cameras: [], microphones: ['same'], system: ['same'] } }));
		const reopened = new ConfigurationStore(path);
		try {
			await reopened.initialize();
			assert.deepEqual(reopened.snapshot.values['media.sources'].cameras, ['camera-one']);
			assert.deepEqual(reopened.snapshot.values['recording.defaults'], limits);
		} finally {
			reopened.dispose();
		}
	} finally {
		store.dispose();
		await rm(dir, { recursive: true, force: true });
	}
});
