import assert from 'node:assert/strict';
import test from 'node:test';
import { fakeEngine, fakeMainProcess } from './fixtures.mjs';
import { build } from 'esbuild';
import { mkdtemp, mkdir, readFile, writeFile, readdir, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

const output = await build({
	stdin: {
		contents: `
			export { UserSettings } from './src/platform/configuration/node/userSettings.js';
			export { EngineSettings } from './src/platform/configuration/electron-main/engineSettings.js';
			export { settingDefaults, settingKeys, settingSchema, validateSetting } from './src/platform/configuration/common/configurationSchema.js';
			export { isConfigurationUpdate, isConfigurationSnapshot } from './src/platform/configuration/common/configurationSnapshot.js';
			export { ConfigurationService } from './src/workbench/services/configuration/configurationServiceImpl.js';
			export { validateRelays } from './src/platform/engine/common/relayValidation.js';`,
		resolveDir: process.cwd(), loader: 'ts',
	}, bundle: true, write: false, platform: 'node', format: 'esm', tsconfig: 'tsconfig.json',
});

const { UserSettings, EngineSettings, settingDefaults, settingKeys, settingSchema, validateSetting, isConfigurationUpdate, isConfigurationSnapshot, ConfigurationService, validateRelays } = await import(`data:text/javascript;base64,${Buffer.from(output.outputFiles[0].text).toString('base64')}`);

async function withSettingsFile(run) {
	const dir = await mkdtemp(join(tmpdir(), 'vnilink-settings-'));
	try {
		await run(join(dir, 'settings.json'), dir);
	} finally {
		await rm(dir, { recursive: true, force: true });
	}
}

test('every setting is titled, has a valid default and refuses values outside its schema', () => {
	for (const key of settingKeys()) {
		const schema = settingSchema(key);
		assert.ok(schema.title, `${key} has no title`);
		assert.equal(validateSetting(key, schema.default), undefined, `${key} default is invalid`);
	}
	assert.match(validateSetting('appearance.theme', 'sepia'), /available options/);
	assert.match(validateSetting('power.incoming.idleTimeout', 1441), /0 to 1440/);
	assert.match(validateSetting('power.incoming.idleTimeout', 1 / 7), /whole seconds/);
	assert.equal(validateSetting('power.incoming.idleTimeout', 1.5), undefined);
	assert.match(validateSetting('terminal.maxSessions', 2.5), /whole number/);
	assert.match(validateSetting('desktop.quality', { max_bitrate_kbps: 1, max_frame_rate: 61, max_width: 1, max_height: 1 }), /60 fps/);
	// Automatic is the default: the bandwidth limit is the most a stream holds.
	assert.deepEqual(settingDefaults('user')['desktop.quality'], { max_bitrate_kbps: 50000, max_frame_rate: 30, max_width: 1920, max_height: 1080 });
	assert.match(validateSetting('future.setting', true), /Unknown setting/);
	assert.equal(settingKeys().includes('appearance.nativeBackground'), false);
});

test('updates and snapshots crossing IPC are strict, and snapshots never carry relay tokens', () => {
	assert.equal(isConfigurationUpdate({ key: 'appearance.theme', value: 'dark' }), true);
	assert.equal(isConfigurationUpdate({ key: 'appearance.theme', value: 'invalid' }), false);
	assert.equal(isConfigurationUpdate({ key: 'appearance.theme', value: 'dark', extra: true }), false);
	const snapshot = { revision: 1, values: settingDefaults('user'), errors: { power: 'Unavailable' }, restricted: ['security.allowRestart'] };
	assert.equal(isConfigurationSnapshot(snapshot), true);
	assert.equal(isConfigurationSnapshot({ ...snapshot, revision: -1 }), false);
	assert.equal(isConfigurationSnapshot({ ...snapshot, errors: { future: 'x' } }), false);
	assert.equal(isConfigurationSnapshot({ ...snapshot, values: { 'connection.relays': [{ url: 'https://relay.example/', authenticated: true, replacementToken: 'secret' }] } }), false);
	assert.equal(isConfigurationSnapshot({ ...snapshot, values: { 'connection.relays': [{ url: 'https://relay.example/', authenticated: true }] } }), true);
});

test('settings.json holds only changed settings, keeps unknown keys and survives reopen', () => withSettingsFile(async (path, dir) => {
	await writeFile(path, JSON.stringify({ 'future.setting': { kept: true } }));
	const settings = new UserSettings(path);
	let changes = 0;
	settings.onDidChange(() => changes++);
	try {
		await settings.initialize();
		assert.deepEqual(settings.values, settingDefaults('user'));
		await Promise.all([settings.update('appearance.theme', 'dark'), settings.update('input.swapModifiers', true)]);
		assert.equal(changes, 2);
		assert.deepEqual(JSON.parse(await readFile(path, 'utf8')), { 'future.setting': { kept: true }, 'appearance.theme': 'dark', 'input.swapModifiers': true });
		await settings.update('input.swapModifiers', false);
		assert.deepEqual(JSON.parse(await readFile(path, 'utf8')), { 'future.setting': { kept: true }, 'appearance.theme': 'dark' });
		assert.deepEqual(await readdir(dir), ['settings.json']);
		const quality = { max_bitrate_kbps: 5000, max_frame_rate: 30, max_width: 1920, max_height: 1080 };
		const writing = settings.update('desktop.quality', quality);
		quality.max_frame_rate = 1;
		await writing;
		assert.equal(settings.values['desktop.quality'].max_frame_rate, 30, 'A caller changed a queued value');
		assert.ok(Object.isFrozen(settings.values['desktop.quality']));
		const reopened = new UserSettings(path);
		try {
			await reopened.initialize();
			assert.equal(reopened.values['appearance.theme'], 'dark');
			assert.equal(reopened.values['desktop.quality'].max_bitrate_kbps, 5000);
		} finally {
			reopened.dispose();
		}
	} finally {
		settings.dispose();
	}
}));

test('a settings file that cannot be read is preserved and never overwritten', () => withSettingsFile(async path => {
	for (const contents of ['{broken', '{"appearance.theme":"unknown"}', '{"power.lockAfterControl":true}', '[]']) {
		await writeFile(path, contents);
		const settings = new UserSettings(path);
		try {
			await settings.initialize();
			assert.ok(settings.error, `${contents} was accepted`);
			assert.deepEqual(settings.values, settingDefaults('user'));
			await assert.rejects(settings.update('appearance.theme', 'light'));
			assert.equal(await readFile(path, 'utf8'), contents);
		} finally {
			settings.dispose();
		}
	}
}));

test('failed atomic writes keep effective values and later writes recover', () => withSettingsFile(async (path, dir) => {
	const settings = new UserSettings(path);
	try {
		await settings.initialize();
		await mkdir(path);
		let changes = 0;
		settings.onDidChange(() => changes++);
		await assert.rejects(settings.update('appearance.theme', 'dark'));
		assert.equal(changes, 0);
		assert.equal(settings.values['appearance.theme'], 'system');
		assert.deepEqual(await readdir(dir), ['settings.json']);
		await rm(path, { recursive: true });
		await settings.update('appearance.theme', 'light');
		assert.equal(changes, 1);
		assert.equal(settings.values['appearance.theme'], 'light');
	} finally {
		settings.dispose();
	}
}));

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

/** An engine holding each policy, with switches to refuse writes or reads. */
function policyEngine() {
	const state = {
		power: {
			incoming_display: false, incoming_system: true, outgoing_display: false, outgoing_system: false,
			incoming_idle_secs: 90, outgoing_idle_secs: 0, lock_after_control: false,
		},
		terminal: {
			allow_persistence: true, max_sessions: 16, max_sessions_per_peer: 16, history_bytes: 1048576, replay_bytes: 8192, detached_timeout_secs: 300,
		},
		sharing: {
			view: false, control: false, clipboard: true, files: false, terminal: false,
			camera: false, system_audio: false, microphone: false, recording: false, system_actions: false,
		},
		system: { allow_lock: true, allow_restart: false, allow_shutdown: false, allow_privacy: false },
		support: { lock: true, restart: false, shutdown: true, privacy: true },
		relays: [],
		failWrite: false, failRead: false, restartRefreshes: 0, commands: [],
	};
	const engine = fakeEngine({
		async engineCommand(command) {
			state.commands.push(command);
			if (command.type.startsWith('set') || command.type === 'saveCustomRelays') {
				if (state.failWrite) {
					throw new Error('Engine refused');
				}
			} else if (state.failRead) {
				throw new Error('Engine stopped');
			}
			switch (command.type) {
				case 'powerPolicy': return { type: 'powerPolicy', policy: state.power };
				case 'terminalPolicy': return { type: 'terminalPolicy', policy: state.terminal };
				case 'sharingPolicy': return { type: 'sharingPolicy', policy: state.sharing };
				case 'systemPolicy': return { type: 'systemPolicy', policy: state.system, support: state.support };
				case 'savedRelays': return { type: 'relays', relays: state.relays };
				case 'setPowerPolicy': state.power = command.policy; break;
				case 'setTerminalPolicy': state.terminal = command.policy; break;
				case 'setSharingPolicy': state.sharing = { ...state.sharing, ...command.policy }; break;
				case 'setSystemPolicy': state.system = command.policy; break;
				case 'saveCustomRelays': state.relays = command.relays.map(({ url, authenticated }) => ({ url, authenticated })); break;
				case 'setRelayPublic': state.relays = []; break;
			}
			return { type: 'done' };
		},
	});
	engine.refreshRestartRequired = async () => {
		state.restartRefreshes++;
	};
	return { engine, state };
}

test('engine policies read whole and write through a read-modify-write with read-back', async () => {
	const { engine, state } = policyEngine();
	const settings = new EngineSettings(engine);
	try {
		await settings.refresh();
		assert.equal(settings.values['power.incoming.idleTimeout'], 1.5);
		assert.equal(settings.values['terminal.historySize'], 1024);
		assert.deepEqual(settings.sourceErrors, {});
		await settings.update('power.outgoing.idleTimeout', 2.5);
		assert.equal(state.power.outgoing_idle_secs, 150);
		assert.equal(state.power.incoming_idle_secs, 90, 'Another field of the policy changed');
		await settings.update('terminal.replaySize', 16);
		assert.equal(state.terminal.replay_bytes, 16384);
		await assert.rejects(settings.update('terminal.maxSessions', 4), /Per computer can’t be more than Terminals at once/);
		assert.equal(state.terminal.max_sessions, 16, 'A limit below the per-device limit reached the engine');
		await settings.update('terminal.maxSessionsPerDevice', 4);
		await settings.update('terminal.maxSessions', 4);
		await assert.rejects(settings.update('terminal.replaySize', 2048), /Replay can’t be larger than the history limit/);
		assert.deepEqual([state.terminal.max_sessions, state.terminal.max_sessions_per_peer], [4, 4]);
		await settings.update('sharing.screen', true);
		assert.deepEqual(state.commands.filter(command => command.type === 'setSharingPolicy').at(-1).policy, { view: true, control: true, system_audio: true }, 'Screen sharing did not bring its sound');
		assert.equal(state.sharing.clipboard, true, 'Sharing write replaced unrelated consent');
		assert.equal(settings.values['sharing.screen'], true);
	} finally {
		settings.dispose();
	}
});

test('unsupported actions are restricted and cannot be enabled; supported ones keep the other rules', async () => {
	const { engine, state } = policyEngine();
	const settings = new EngineSettings(engine);
	try {
		await settings.refresh();
		assert.deepEqual(settings.restricted, ['security.allowRestart']);
		await assert.rejects(settings.update('security.allowRestart', true), /not supported/);
		assert.ok(!state.commands.some(command => command.type === 'setSystemPolicy'));
		await settings.update('security.allowShutdown', true);
		assert.deepEqual(state.system, { allow_lock: true, allow_restart: false, allow_shutdown: true, allow_privacy: false });
	} finally {
		settings.dispose();
	}
});

test('relay credentials are write-only, saved tokens are kept for their URL, and relay changes refresh restart state', async () => {
	const { engine, state } = policyEngine();
	const settings = new EngineSettings(engine);
	try {
		await settings.refresh();
		await assert.rejects(settings.update('connection.relays', [relay('https://relay.example/', true)]), /token/);
		assert.ok(!state.commands.some(command => command.type === 'saveCustomRelays'), 'An invalid relay list reached the engine');
		await settings.update('connection.relays', [relay('https://relay.example/', true, 'secret')]);
		assert.equal(state.commands.find(command => command.type === 'saveCustomRelays').relays[0].replacementToken, 'secret');
		assert.ok(!JSON.stringify(settings.values).includes('secret'), 'A saved token was published');
		await settings.update('connection.relays', [{ url: 'https://relay.example/', authenticated: true }]);
		assert.equal(state.commands.filter(command => command.type === 'saveCustomRelays').at(-1).relays[0].replacementToken, '');
		await settings.update('connection.relays', []);
		assert.equal(state.commands.at(-2).type, 'setRelayPublic');
		assert.equal(state.restartRefreshes, 3);
	} finally {
		settings.dispose();
	}
});

test('failed writes publish what the engine holds; failed read-backs and stopped engines publish no values', async () => {
	const { engine, state } = policyEngine();
	const settings = new EngineSettings(engine);
	try {
		await settings.refresh();
		state.failWrite = true;
		await assert.rejects(settings.update('sharing.files', true), /could not be changed/);
		assert.equal(settings.values['sharing.files'], false);
		state.failWrite = false;
		const original = engine.execute;
		let written = false;
		engine.execute = async command => {
			if (command.type === 'sharingPolicy' && written) {
				throw new Error('Read lost');
			}
			written ||= command.type === 'setSharingPolicy';
			return original(command);
		};
		await assert.rejects(settings.update('sharing.files', true), /current state could not be read/);
		assert.equal('sharing.files' in settings.values, false);
		assert.match(settings.sourceErrors.sharing, /could not be loaded/);
		engine.execute = original;
		engine.setAvailable(false);
		await settings.refresh();
		assert.deepEqual(settings.values, {});
		assert.match(settings.sourceErrors.power, /unavailable/);
		await assert.rejects(settings.update('power.lockAfterControl', true), /unavailable/);
		engine.setAvailable(true);
		await new Promise(resolve => setTimeout(resolve, 0));
		assert.equal(settings.values['power.lockAfterControl'], false, 'A restarted engine was not read again');
	} finally {
		settings.dispose();
	}
});

test('window configuration ignores stale snapshots, reports readability changes and releases its subscription', async () => {
	let finish;
	let receive;
	let unsubscribed = 0;
	const snapshot = (revision, values = {}, errors = {}) => ({ revision, values: { ...settingDefaults('user'), ...values }, errors, restricted: [] });
	const service = new ConfigurationService(fakeMainProcess({
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
	const events = [];
	service.onDidChangeConfiguration(event => events.push([...event.affectedKeys]));
	const loading = service.initialize();
	const quality = { max_bitrate_kbps: 2000, max_frame_rate: 30, max_width: 1920, max_height: 1080 };
	receive(snapshot(2, { 'appearance.theme': 'dark', 'desktop.quality': quality }, { power: 'Unavailable' }));
	quality.max_frame_rate = 10;
	assert.equal(service.getValue('desktop.quality').max_frame_rate, 30);
	assert.equal(service.getUnavailableReason('power.lockAfterControl'), 'Unavailable');
	finish(snapshot(0));
	await loading;
	assert.equal(service.getValue('appearance.theme'), 'dark');
	receive(snapshot(3, { 'appearance.theme': 'dark', 'desktop.quality': { ...quality, max_frame_rate: 30 } }));
	assert.ok(events.at(-1).includes('power.lockAfterControl') && !events.at(-1).includes('appearance.theme'), 'Readability change was not reported alone');
	await assert.rejects(service.updateValue('appearance.theme', 'light'));
	assert.equal(service.getValue('appearance.theme'), 'dark');
	service.dispose();
	assert.equal(unsubscribed, 1);
	receive(snapshot(4));
	assert.equal(service.getValue('appearance.theme'), 'dark');
});
