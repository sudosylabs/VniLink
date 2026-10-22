import assert from 'node:assert/strict';
import test from 'node:test';
import { build } from 'esbuild';
import { fileURLToPath } from 'node:url';

const bundle = await build({ entryPoints: [fileURLToPath(new URL('../../src/platform/engine/common/engine.ts', import.meta.url))], bundle: true, write: false, format: 'esm', platform: 'node' });

const { isEngineCommand, isEngineReply, isEngineEvent } = await import(`data:text/javascript;base64,${Buffer.from(bundle.outputFiles[0].text).toString('base64')}`);

test('control commands reject privileged methods, malformed IDs and extra authority', () => {
	assert.equal(isEngineCommand({ type: 'shutdown' }), false);
	assert.equal(isEngineCommand({ type: 'snapshot', extra: true }), false);
	assert.equal(isEngineCommand({ type: 'confirmPairing', previewId: '18446744073709551615', digits: '123' }), true);
	assert.equal(isEngineCommand({ type: 'confirmPairing', previewId: '18446744073709551616', digits: '123' }), false);
	assert.equal(isEngineCommand({ type: 'confirmPairing', previewId: 1, digits: '123' }), false);
	assert.equal(isEngineCommand({ type: 'confirmPairing', previewId: '1', digits: '123456' }), false);
	assert.equal(isEngineCommand({ type: 'openPairingFile', bytes: [256] }), false);
	assert.equal(isEngineCommand({ type: 'createInvitation', ttlSeconds: 60 }), true);
	assert.equal(isEngineCommand({ type: 'createInvitation', ttlSeconds: NaN }), false);
	assert.equal(isEngineCommand({ type: 'authorize', endpointId: 'peer', view: true, control: true, clipboard: false, transfer: false }), false);
	assert.equal(isEngineCommand({ type: 'authorize', endpointId: 'peer', view: true, control: true, clipboard: false }), false);
	assert.equal(isEngineCommand({ type: 'connect', endpointId: 'peer' }), true);
	assert.equal(isEngineCommand({ type: 'beginConnect', endpointId: 'peer' }), true);
	assert.equal(isEngineCommand({ type: 'beginConnectServices', endpointId: 'peer' }), true);
	assert.equal(isEngineCommand({ type: 'beginConnectServices' }), false);
	assert.equal(isEngineCommand({ type: 'beginConnectServices', endpointId: 'peer', allowed: true }), false);
	assert.equal(isEngineCommand({ type: 'setTerminalEnabled', enabled: true }), false);
	assert.equal(isEngineCommand({ type: 'authorizeTerminal', endpointId: 'peer', allowed: true }), false);
	assert.equal(isEngineCommand({ type: 'connect' }), false);
	assert.equal(isEngineCommand({ type: 'disconnect' }), true);
	assert.equal(isEngineCommand({ type: 'disconnectSession', sessionId: '1' }), true);
	assert.equal(isEngineCommand({ type: 'disconnectSession', sessionId: '18446744073709551616' }), false);
	assert.equal(isEngineCommand({ type: 'openPairingFileBase64', data: Buffer.from([1, 2, 3]).toString('base64') }), true);
	assert.equal(isEngineCommand({ type: 'openPairingFileBase64', data: '!!!' }), false);
});

test('identity, codecs, relays and session settings validate exactly', () => {
	assert.equal(isEngineCommand({ type: 'endpoint' }), true);
	assert.equal(isEngineCommand({ type: 'endpoint', extra: true }), false);
	assert.equal(isEngineCommand({ type: 'capabilities' }), true);
	assert.equal(isEngineCommand({ type: 'videoCodecs' }), true);
	assert.equal(isEngineCommand({ type: 'clipboardOwnerSession' }), true);
	assert.equal(isEngineCommand({ type: 'subscribeEvents' }), true);
	assert.equal(isEngineCommand({ type: 'subscribeVideo' }), true);
	const codecs = { encode_baseline: true, encode_high: false, decode_baseline: true, decode_high: false, max_width: 4096, max_height: 4096, hardware: true, device: 'test', chroma444: false };
	assert.equal(isEngineCommand({ type: 'setVideoCodecs', codecs }), true);
	assert.equal(isEngineCommand({ type: 'setVideoCodecs', codecs: { ...codecs, max_width: 65536 } }), false);
	assert.equal(isEngineCommand({ type: 'setVideoCodecs', codecs: { ...codecs, extra: 1 } }), false);
	assert.equal(isEngineCommand({ type: 'saveCustomRelays', relays: [{ url: 'https://relay.example', authenticated: false, replacementToken: '' }] }), true);
	assert.equal(isEngineCommand({ type: 'saveCustomRelays', relays: [{ url: 'https://relay.example', authenticated: true, replacementToken: 'token' }] }), true);
	assert.equal(isEngineCommand({ type: 'saveCustomRelays', relays: [{ url: 'https://relay.example', authenticated: true }] }), false);
	assert.equal(isEngineCommand({ type: 'saveCustomRelays', relays: new Array(17).fill({ url: 'https://relay.example', authenticated: false, replacementToken: '' }) }), false);
	assert.equal(isEngineCommand({ type: 'peerHostname', endpointId: 'peer' }), true);
	assert.equal(isEngineCommand({ type: 'peerHostname' }), false);
	assert.equal(isEngineCommand({ type: 'connectFiles', endpointId: 'peer' }), false, 'Files rides the services connection');
	assert.equal(isEngineCommand({ type: 'sessionKeyboardMode', sessionId: '1', generation: '2' }), true);
	assert.equal(isEngineCommand({ type: 'setSessionKeyboardMode', sessionId: '1', generation: '2', mode: 'Map' }), true);
	assert.equal(isEngineCommand({ type: 'setSessionKeyboardMode', sessionId: '1', generation: '2', mode: 'Dvorak' }), false);
	assert.equal(isEngineCommand({ type: 'sessionSwapControlAndCommand', sessionId: '1', generation: '2' }), true);
	assert.equal(isEngineCommand({ type: 'setSessionSwapControlAndCommand', sessionId: '1', generation: '2', enabled: true }), true);
	assert.equal(isEngineCommand({ type: 'setSessionSwapControlAndCommand', sessionId: '1', generation: '2', enabled: 'yes' }), false);
	assert.equal(isEngineCommand({ type: 'controllerSessionPaths', sessionId: '1' }), true);
	assert.equal(isEngineCommand({ type: 'controllerSessionPaths', sessionId: 'nope' }), false);
	assert.equal(isEngineCommand({ type: 'pollEvents', subscriptionId: '1', maxEvents: 32 }), true);
	assert.equal(isEngineCommand({ type: 'pollEvents', subscriptionId: '1', maxEvents: 0 }), false);
	assert.equal(isEngineCommand({ type: 'pollEvents', subscriptionId: '1', maxEvents: 257 }), false);
	assert.equal(isEngineCommand({ type: 'releaseEvents', subscriptionId: '1' }), true);
	assert.equal(isEngineReply({ type: 'endpoint', endpointId: 'abc' }), true);
	assert.equal(isEngineReply({ type: 'capabilities', capabilities: { desktop_capture: true, keyboard: true, pointer: true, clipboard: true, file_transfer: true, audio: false, monitors: 1, h264_encode: true, h264_decode: true } }), true);
	assert.equal(isEngineReply({ type: 'videoCodecs', codecs }), true);
	assert.equal(isEngineReply({ type: 'hostname', hostname: null }), true);
	assert.equal(isEngineReply({ type: 'hostname', hostname: 'peer' }), true);
	assert.equal(isEngineReply({ type: 'ownerSession', session: null }), true);
	assert.equal(isEngineReply({ type: 'eventSubscription', subscriptionId: '3' }), true);
	assert.equal(isEngineReply({ type: 'events', events: [], lagged: '0' }), true);
	assert.equal(isEngineReply({ type: 'events', events: [{ type: 'hostingChanged', hosting: true }], lagged: '0' }), true);
	assert.equal(isEngineReply({ type: 'events', events: [{ type: 'bogus' }], lagged: '0' }), false);
});

test('bridge events keep core names and lossless ids', () => {
	assert.equal(isEngineEvent({ type: 'hostingChanged', hosting: false }), true);
	assert.equal(isEngineEvent({ type: 'folderChanged', watch: '4' }), true);
	assert.equal(isEngineEvent({ type: 'folderChanged', watch: 4 }), false, 'ids are decimal strings');
	assert.equal(isEngineEvent({ type: 'devicePaired', endpoint_id: 'peer' }), true);
	assert.equal(isEngineEvent({ type: 'controlRequested', endpoint_id: 'peer', check: '123' }), true);
	assert.equal(isEngineEvent({ type: 'peerRenamed', endpoint_id: 'peer', hostname: null }), true);
	assert.equal(isEngineEvent({ type: 'sessionChanged', id: '7', generation: '9', role: 'Host', phase: 'Active', peer_id: 'peer' }), true);
	assert.equal(isEngineEvent({ type: 'sessionChanged', id: '7', generation: '9', role: 'Host', phase: { Failed: { message: 'lost' } }, peer_id: 'peer' }), true);
	assert.equal(isEngineEvent({ type: 'sessionChanged', id: 7, generation: '9', role: 'Host', phase: 'Active', peer_id: 'peer' }), false);
	const service = { id: '1', connectionId: '2', generation: '3', peer_id: 'peer', kind: 'Desktop', phase: 'Active' };
	assert.equal(isEngineEvent({ type: 'serviceChanged', service }), true);
	assert.equal(isEngineEvent({ type: 'serviceChanged', service: { ...service, kind: 'Hologram' } }), false);
	assert.equal(isEngineEvent({ type: 'serviceChanged', service: { ...service, phase: { Closed: { cause: 'Revoked' } } } }), true);
	assert.equal(isEngineEvent({ type: 'serviceChanged', service: { ...service, phase: { Closed: { cause: 'Abducted' } } } }), false);
	// A producer failure on this computer says what failed, and only it carries one.
	assert.equal(isEngineEvent({ type: 'serviceChanged', service: { ...service, phase: { Closed: { cause: 'Failed', failure: 'encoder' } } } }), true);
	assert.equal(isEngineEvent({ type: 'serviceChanged', service: { ...service, phase: { Closed: { cause: 'Failed' } } } }), false);
	assert.equal(isEngineEvent({ type: 'serviceChanged', service: { ...service, phase: { Closed: { cause: 'Failed', failure: 'boredom' } } } }), false);
	assert.equal(isEngineEvent({ type: 'serviceChanged', service: { ...service, phase: { Closed: { cause: 'Revoked', failure: 'encoder' } } } }), false);
	assert.equal(isEngineEvent({ type: 'qualityUpdated', id: '1', quality: { max_bitrate_kbps: 1000, max_frame_rate: 30, max_width: 1920, max_height: 1080 } }), true);
	assert.equal(isEngineEvent({ type: 'inputFailed', id: '1', message: 'denied' }), true);
	assert.equal(isEngineEvent({ type: 'topologyChanged', revision: '4' }), true);
	assert.equal(isEngineEvent({ type: 'hostLocked', locked: true }), true);
	assert.equal(isEngineEvent({ type: 'hostLocked', locked: true, extra: 1 }), false);
	assert.equal(isEngineEvent({ type: 'nope' }), false);
});

test('service commands validate kinds, policies, sessions and jobs exactly', () => {
	assert.equal(isEngineCommand({ type: 'servicePermissions', endpointId: 'peer' }), false);
	assert.equal(isEngineCommand({ type: 'authorizeService', endpointId: 'peer', kind: 'Terminal', allowed: true }), false);
	assert.equal(isEngineCommand({ type: 'authorizeService', endpointId: 'peer', kind: 'Hologram', allowed: true }), false);
	assert.equal(isEngineCommand({ type: 'authorizeRecording', endpointId: 'peer', allowed: false }), false);
	assert.equal(isEngineCommand({ type: 'terminalPolicy' }), true);
	const policy = {
		allow_persistence: true, max_sessions: 4, max_sessions_per_peer: 2, history_bytes: 65536, replay_bytes: 16384, detached_timeout_secs: 300,
	};
	assert.equal(isEngineCommand({ type: 'setTerminalPolicy', policy }), true);
	assert.equal(isEngineCommand({ type: 'setTerminalPolicy', policy: { ...policy, max_sessions: 65536 } }), false);
	// A window names a detected shell by id; it can no longer install a program as the host shell.
	assert.equal(isEngineCommand({ type: 'terminalShells' }), true);
	assert.equal(isEngineCommand({ type: 'setTerminalShell', shell: 'pwsh' }), true);
	assert.equal(isEngineCommand({ type: 'setTerminalShell', shell: null }), true);
	for (const shell of ['', 'x'.repeat(1025), 'a\0b', 7, undefined]) {
		assert.equal(isEngineCommand({ type: 'setTerminalShell', shell }), false);
	}
	assert.equal(isEngineCommand({ type: 'setTerminalShell', shell: 'pwsh', program: 'C:\\evil.exe' }), false);
	for (const type of ['enableNativeTerminal', 'enableDefaultNativeTerminal', 'disableTerminal']) {
		assert.equal(isEngineCommand({ type, program: '/bin/zsh', directory: '/tmp' }), false);
		assert.equal(isEngineCommand({ type }), false);
	}
	const shells = { type: 'terminalShells', shells: [{ id: '/bin/zsh', label: 'zsh' }, { id: '/bin/bash', label: 'bash' }], automatic: '/bin/zsh', current: null };
	assert.equal(isEngineReply(shells), true);
	assert.equal(isEngineReply({ ...shells, current: '/bin/bash' }), true);
	assert.equal(isEngineReply({ ...shells, shells: [{ id: '/bin/zsh', label: 'zsh', program: '/bin/zsh' }] }), false);
	assert.equal(isEngineReply({ ...shells, automatic: '' }), false);
	assert.equal(isEngineCommand({ type: 'remoteTerminalSessions', connectionId: '1', generation: '2' }), true);
	assert.equal(isEngineCommand({ type: 'closeRemoteTerminalSession', connectionId: '1', generation: '2', id: 'abc' }), true);
	assert.equal(isEngineCommand({ type: 'hostedTerminalSessions' }), true);
	assert.equal(isEngineCommand({ type: 'closeTerminalSession', id: 'abc' }), true);
	assert.equal(isEngineCommand({ type: 'subscribeFilesJobs', sessionId: '1', generation: '2' }), false, 'transfers report on the files channel');
	assert.equal(isEngineCommand({ type: 'localPlaces' }), true);
	assert.equal(isEngineCommand({ type: 'listLocal', root: '0', path: 'Users/hammed', offset: '0' }), true);
	assert.equal(isEngineCommand({ type: 'listLocal', root: '0', path: '../etc', offset: '0' }), false);
	assert.equal(isEngineCommand({ type: 'locateLocal', path: '/Users/hammed/Desktop/a.txt' }), true);
	assert.equal(isEngineCommand({ type: 'locateLocal', path: '' }), false, 'an empty path names nothing');
	assert.equal(isEngineCommand({ type: 'watchLocal', root: '0', path: 'Users/hammed' }), true);
	assert.equal(isEngineCommand({ type: 'unwatchLocal', watch: '3' }), true);
	assert.equal(isEngineCommand({ type: 'unwatchLocal', watch: 'three' }), false);
	assert.equal(isEngineCommand({ type: 'createLocalDir', root: '0', path: '', name: 'New Folder' }), true);
	assert.equal(isEngineCommand({ type: 'renameLocal', root: '0', path: 'Users', name: 'a', token: { dev: '1', ino: '2' }, newName: 'b' }), true);
	assert.equal(isEngineCommand({ type: 'renameLocal', root: '0', path: 'Users', name: 'a', token: { dev: 1, ino: 2 }, newName: 'b' }), false, 'token parts are decimal strings');
	assert.equal(isEngineCommand({ type: 'deleteLocal', root: '0', path: 'Users', name: 'a', token: null, trash: true }), true);
	assert.equal(isEngineCommand({ type: 'deleteLocal', root: '0', path: 'Users', name: 'a', token: null }), false, 'a delete says whether it goes to the Trash');
	assert.equal(isEngineCommand({ type: 'deleteLocal', root: '0', path: 'Users', name: 'a' }), false);
	assert.equal(isEngineReply({ type: 'servicePermissions', permissions: { desktop: true, files: true, terminal: false, camera: false, system_audio: false, microphone: false, recording: false, system_actions: false } }), false);
	assert.equal(isEngineReply({ type: 'terminalPolicy', policy }), true);
	assert.equal(isEngineReply({ type: 'terminalSessions', sessions: [] }), true);
	assert.equal(isEngineReply({ type: 'terminalSessions', sessions: [{ id: 'a', process_id: 42, shell: '/bin/zsh', rows: 24, columns: 80, persistent: true, attached: true }] }), true);
	const places = { roots: [{ id: '2', name: 'C:' }], folders: [{ kind: 'home', root: '2', path: 'Users/abass' }] };
	assert.equal(isEngineReply({ type: 'places', places }), true);
	assert.equal(isEngineReply({ type: 'location', location: { root: '0', path: 'Users/hammed', name: 'a.txt' } }), true);
	assert.equal(isEngineReply({ type: 'location', location: null }), true, 'a path outside what is shared');
	assert.equal(isEngineReply({ type: 'watching', watch: '3' }), true);
	assert.equal(isEngineReply({ type: 'location', location: { root: '0', path: '/Users', name: 'a.txt' } }), false);
	assert.equal(isEngineReply({ type: 'places', places: { ...places, folders: [{ kind: 'home', root: '2', path: '/Users' }] } }), false);
	const entry = { name: 'a.txt', kind: 'file', size: '3', mtimeMs: '1700000000000', token: { dev: '1', ino: '9' } };
	const page = { entries: [entry], total: '1', nextOffset: null, truncated: false, skipped: '0' };
	assert.equal(isEngineReply({ type: 'dirPage', page }), true);
	assert.equal(isEngineReply({ type: 'dirPage', page: { ...page, entries: [{ ...entry, name: '..' }] } }), false);
});

test('pairing handles cross the bridge as lossless decimal strings', () => {
	const preview = { id: '18446744073709551615', endpoint_id: 'peer', check: '123', expires_at_ms: '18446744073709551615' };
	assert.equal(isEngineReply({ type: 'pairingPreview', preview }), true);
	assert.equal(isEngineReply({ type: 'pairingPreview', preview: { ...preview, id: Number(preview.id) } }), false);
	assert.equal(isEngineReply({ type: 'device', device: { endpoint_id: 'peer' } }), false);
});

test('global sharing policy rejects missing fields, extras and peer-scoped writes', () => {
	const policy = {
		view: false, control: false, clipboard: false, files: false, terminal: false,
		camera: false, system_audio: false, microphone: false, recording: false, system_actions: false,
	};
	assert.equal(isEngineCommand({ type: 'sharingPolicy' }), true);
	assert.equal(isEngineCommand({ type: 'setSharingPolicy', policy }), true);
	assert.equal(isEngineReply({ type: 'sharingPolicy', policy }), true);
	assert.equal(isEngineCommand({ type: 'setSharingPolicy', policy: { ...policy, camera: 'yes' } }), false);
	assert.equal(isEngineCommand({ type: 'setSharingPolicy', policy, endpointId: 'peer' }), false);
	assert.equal(isEngineCommand({ type: 'setSharingPolicy', policy: { ...policy, extra: true } }), false);
});

test('presence, peer availability and hosted services cross the bridge exactly', () => {
	assert.equal(isEngineEvent({ type: 'presenceChanged', endpoint_id: 'peer', presence: 'notAccepting' }), true);
	assert.equal(isEngineEvent({ type: 'presenceChanged', endpoint_id: 'peer', presence: 'asleep' }), false);
	assert.equal(isEngineEvent({ type: 'peerAvailabilityChanged', endpoint_id: 'peer' }), true);
	assert.equal(isEngineEvent({ type: 'peerAvailabilityChanged', endpoint_id: 'peer', extra: 1 }), false);
	assert.equal(isEngineCommand({ type: 'peerAvailability', endpointId: 'peer' }), true);
	assert.equal(isEngineCommand({ type: 'peerAvailability' }), false);
	assert.equal(isEngineCommand({ type: 'hostedServices' }), true);
	assert.equal(isEngineReply({ type: 'peerAvailability', report: null }), true);
	assert.equal(isEngineReply({ type: 'peerAvailability', report: [{ capability: 'camera', status: 'turnedOff' }, { capability: 'files', status: 'available' }] }), true);
	assert.equal(isEngineReply({ type: 'peerAvailability', report: [{ capability: 'camera', status: 'off' }] }), false, 'Unknown status');
	assert.equal(isEngineReply({ type: 'peerAvailability', report: [{ capability: 'webcam', status: 'available' }] }), false, 'Unknown capability');
	assert.equal(isEngineReply({ type: 'peerAvailability', report: [{ capability: 'camera', status: 'available' }, { capability: 'camera', status: 'turnedOff' }] }), false, 'Repeated capability');
	assert.equal(isEngineReply({ type: 'services', services: [] }), true);
	assert.equal(isEngineReply({ type: 'services', services: [{ id: '1' }] }), false);
});
