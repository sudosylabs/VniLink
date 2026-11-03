// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'assert';
import { ensureNoDisposablesAreLeakedInTestSuite } from '../../../../base/test/common/utils.js';
import { isEngineCommand, isEngineEvent, isEngineReply, nativeCapabilityNames } from '../../common/engine.js';

/** A guard case: what the value is, and the value. */
type Case = readonly [name: string, value: unknown];

/** One test per case, each checking that `guard` lets the value cross the bridge. */
function accepts(guard: (value: unknown) => boolean, cases: readonly Case[]): void {
	for (const [name, value] of cases) {
		test(`${name} is accepted`, () => {
			assert.strictEqual(guard(value), true);
		});
	}
}

/** One test per case, each checking that `guard` stops the value at the bridge. */
function refuses(guard: (value: unknown) => boolean, cases: readonly Case[]): void {
	for (const [name, value] of cases) {
		test(`${name} is refused`, () => {
			assert.strictEqual(guard(value), false);
		});
	}
}

const codecs = { encode_baseline: true, encode_high: false, decode_baseline: true, decode_high: false, max_width: 4096, max_height: 4096, hardware: true, device: 'test', chroma444: false };

const terminalPolicy = {
	allow_persistence: true, max_sessions: 4, max_sessions_per_peer: 2, history_bytes: 65536, replay_bytes: 16384, detached_timeout_secs: 300,
};

const sharingPolicy = {
	view: false, control: false, clipboard: false, files: false, terminal: false,
	camera: false, system_audio: false, microphone: false, recording: false, system_actions: false,
};

const service = { id: '1', connectionId: '2', generation: '3', peer_id: 'peer', kind: 'Desktop', phase: 'Active' };

const shells = { type: 'terminalShells', shells: [{ id: '/bin/zsh', label: 'zsh' }, { id: '/bin/bash', label: 'bash' }], automatic: '/bin/zsh', current: null };

const places = { roots: [{ id: '2', name: 'C:' }], folders: [{ kind: 'home', root: '2', path: 'Users/someone' }] };

const entry = { name: 'a.txt', kind: 'file', size: '3', mtimeMs: '1700000000000', token: { kind: 'identity', dev: '1', ino: '9', born: '7', fingerprint: null } };

const page = { entries: [entry], total: '1', nextOffset: null, truncated: false, skipped: '0' };

const preview = { id: '18446744073709551615', endpoint_id: 'peer', check: '123456', expires_at_ms: '18446744073709551615' };

const recorderLine = `${JSON.stringify({ ms: 1, level: 'INFO', target: 'vnilink::session', message: 'desktop ended' })}\n`;

const diagnosticSnapshot = JSON.stringify({ version: 4, pipelines: [], last_end: null });

const runtime = { type: 'nativeRuntime', backends: [{ name: 'terminal', ready: true, error: null }], capabilities: nativeCapabilityNames.map(name => ({ name, available: name === 'terminal', summary: `${name}: available` })) };

/** A rename of `a` in a FAT volume's folder, naming the entry by `token`. */
const rename = (token: unknown) => ({ type: 'renameLocal', root: '0', path: 'Volumes/STICK', name: 'a', token, newName: 'b' });

suite('isEngineCommand', () => {
	ensureNoDisposablesAreLeakedInTestSuite();
	suite('control', () => {
		accepts(isEngineCommand, [
			['a pairing confirmation with the largest preview id', { type: 'confirmPairing', previewId: '18446744073709551615', digits: '123456' }],
			['an invitation with a lifetime', { type: 'createInvitation', ttlSeconds: 60 }],
			['a services connection to a device', { type: 'beginConnectServices', endpointId: 'peer' }],
			['disconnecting everything', { type: 'disconnect' }],
			['disconnecting one session', { type: 'disconnectSession', sessionId: '1' }],
			['a base64 pairing file', { type: 'openPairingFileBase64', data: 'AQID' }],
		]);
		refuses(isEngineCommand, [
			['shutdown, which only main sends', { type: 'shutdown' }],
			['a command with an extra field', { type: 'snapshot', extra: true }],
			['a preview id beyond 64 bits', { type: 'confirmPairing', previewId: '18446744073709551616', digits: '123456' }],
			['a numeric preview id', { type: 'confirmPairing', previewId: 1, digits: '123456' }],
			['a pairing check that is not six digits', { type: 'confirmPairing', previewId: '1', digits: '123' }],
			['a pairing file byte above 255', { type: 'openPairingFile', bytes: [256] }],
			['an invitation lifetime that is not a number', { type: 'createInvitation', ttlSeconds: NaN }],
			['a per-device grant with every field', { type: 'authorize', endpointId: 'peer', view: true, control: true, clipboard: false, transfer: false }],
			['a per-device grant', { type: 'authorize', endpointId: 'peer', view: true, control: true, clipboard: false }],
			['a desktop connection, since a desktop rides the services connection,', { type: 'connect', endpointId: 'peer' }],
			['a desktop connection begun on its own, since a desktop rides the services connection,', { type: 'beginConnect', endpointId: 'peer' }],
			['a services connection without a device', { type: 'beginConnectServices' }],
			['a services connection that grants something', { type: 'beginConnectServices', endpointId: 'peer', allowed: true }],
			['turning the terminal on', { type: 'setTerminalEnabled', enabled: true }],
			['a per-device terminal grant', { type: 'authorizeTerminal', endpointId: 'peer', allowed: true }],
			['a session id beyond 64 bits', { type: 'disconnectSession', sessionId: '18446744073709551616' }],
			['a pairing file that is not base64', { type: 'openPairingFileBase64', data: '!!!' }],
		]);
	});
	suite('identity, codecs, relays and session settings', () => {
		accepts(isEngineCommand, [
			['an endpoint query', { type: 'endpoint' }],
			['a capabilities query', { type: 'capabilities' }],
			['a codecs query', { type: 'videoCodecs' }],
			['an event subscription', { type: 'subscribeEvents' }],
			['codecs within bounds', { type: 'setVideoCodecs', codecs }],
			['a relay without credentials', { type: 'saveCustomRelays', relays: [{ url: 'https://relay.example', authenticated: false, replacementToken: '' }] }],
			['an authenticated relay with a token', { type: 'saveCustomRelays', relays: [{ url: 'https://relay.example', authenticated: true, replacementToken: 'token' }] }],
			['a device host name query', { type: 'peerHostname', endpointId: 'peer' }],
			['a computer name query', { type: 'computerName' }],
			['a chosen computer name', { type: 'setComputerName', name: 'Studio Mac' }],
			['going back to the system\'s computer name', { type: 'setComputerName', name: null }],
			['a controller session\'s paths', { type: 'controllerSessionPaths', sessionId: '1' }],
			['an event read of 32', { type: 'pollEvents', subscriptionId: '1', maxEvents: 32 }],
			['releasing an event subscription', { type: 'releaseEvents', subscriptionId: '1' }],
		]);
		refuses(isEngineCommand, [
			['an endpoint query with an extra field', { type: 'endpoint', extra: true }],
			['a clipboard owner query, since no session owns the clipboard,', { type: 'clipboardOwnerSession' }],
			...['subscribeVideo', 'subscribeSessionVideo', 'pollVideo', 'releaseVideo', 'publishVideo'].map((type): Case => [`${type}, since pictures go on a desktop's data channel,`, { type }]),
			['codecs wider than 65535', { type: 'setVideoCodecs', codecs: { ...codecs, max_width: 65536 } }],
			['codecs with an extra field', { type: 'setVideoCodecs', codecs: { ...codecs, extra: 1 } }],
			['an authenticated relay without a token field', { type: 'saveCustomRelays', relays: [{ url: 'https://relay.example', authenticated: true }] }],
			['seventeen relays', { type: 'saveCustomRelays', relays: new Array(17).fill({ url: 'https://relay.example', authenticated: false, replacementToken: '' }) }],
			['a host name query without a device', { type: 'peerHostname' }],
			['a files connection, since Files rides the services connection,', { type: 'connectFiles', endpointId: 'peer' }],
			['a session keyboard mode, since a session carries no input to configure,', { type: 'setSessionKeyboardMode', sessionId: '1', generation: '2', mode: 'Map' }],
			['session pointer settings, since a session carries no input to configure,', { type: 'setSessionPointerSettings', sessionId: '1', generation: '2', settings: { sensitivity_percent: 100, reverse_wheel: false, swap_buttons: false } }],
			['a controller session id that is not decimal', { type: 'controllerSessionPaths', sessionId: 'nope' }],
			['an event read of none', { type: 'pollEvents', subscriptionId: '1', maxEvents: 0 }],
			['an event read of more than 256', { type: 'pollEvents', subscriptionId: '1', maxEvents: 257 }],
		]);
	});
	suite('services', () => {
		accepts(isEngineCommand, [
			['a terminal policy query', { type: 'terminalPolicy' }],
			['a terminal policy within bounds', { type: 'setTerminalPolicy', policy: terminalPolicy }],
			['a detected shells query', { type: 'terminalShells' }],
			['a shell named by id', { type: 'setTerminalShell', shell: 'pwsh' }],
			['the automatic shell', { type: 'setTerminalShell', shell: null }],
			['a remote terminal sessions query', { type: 'remoteTerminalSessions', connectionId: '1', generation: '2' }],
			['closing a remote terminal session', { type: 'closeRemoteTerminalSession', connectionId: '1', generation: '2', id: 'abc' }],
			['a hosted terminal sessions query', { type: 'hostedTerminalSessions' }],
			['closing a hosted terminal session', { type: 'closeTerminalSession', id: 'abc' }],
		]);
		refuses(isEngineCommand, [
			['a per-device service permissions query', { type: 'servicePermissions', endpointId: 'peer' }],
			['a per-device service grant', { type: 'authorizeService', endpointId: 'peer', kind: 'Terminal', allowed: true }],
			['a grant of an unknown service', { type: 'authorizeService', endpointId: 'peer', kind: 'Hologram', allowed: true }],
			['a per-device recording grant', { type: 'authorizeRecording', endpointId: 'peer', allowed: false }],
			['a terminal policy above its session limit', { type: 'setTerminalPolicy', policy: { ...terminalPolicy, max_sessions: 65536 } }],
			['an empty shell id', { type: 'setTerminalShell', shell: '' }],
			['a shell id longer than 1024', { type: 'setTerminalShell', shell: 'x'.repeat(1025) }],
			['a shell id with a NUL', { type: 'setTerminalShell', shell: 'a\0b' }],
			['a numeric shell id', { type: 'setTerminalShell', shell: 7 }],
			['a missing shell id', { type: 'setTerminalShell', shell: undefined }],
			['a shell that names its own program', { type: 'setTerminalShell', shell: 'pwsh', program: 'C:\\evil.exe' }],
			...['enableNativeTerminal', 'enableDefaultNativeTerminal', 'disableTerminal'].flatMap((type): Case[] => [
				[`${type} with a program`, { type, program: '/bin/zsh', directory: '/tmp' }],
				[type, { type }],
			]),
			['a files jobs subscription, since transfers report on the files channel,', { type: 'subscribeFilesJobs', sessionId: '1', generation: '2' }],
		]);
	});
	suite('local files', () => {
		accepts(isEngineCommand, [
			['a places query', { type: 'localPlaces' }],
			['a listing under a root', { type: 'listLocal', root: '0', path: 'Users/someone', offset: '0' }],
			['locating an absolute path', { type: 'locateLocal', path: '/Users/someone/Desktop/a.txt' }],
			['a folder watch', { type: 'watchLocal', root: '0', path: 'Users/someone' }],
			['ending a watch', { type: 'unwatchLocal', watch: '3' }],
			['a new folder at a root', { type: 'createLocalDir', root: '0', path: '', name: 'New Folder' }],
			['a rename by identity', { type: 'renameLocal', root: '0', path: 'Users', name: 'a', token: { kind: 'identity', dev: '1', ino: '2', born: '7', fingerprint: null }, newName: 'b' }],
			['a FAT rename by fingerprint', rename({ kind: 'fingerprint', dev: '4', ino: null, born: null, fingerprint: '18446744073709551615' })],
			['a delete that says it goes to the Trash', { type: 'deleteLocal', root: '0', path: 'Users', name: 'a', token: null, trash: true }],
		]);
		refuses(isEngineCommand, [
			['a listing that climbs out of its root', { type: 'listLocal', root: '0', path: '../etc', offset: '0' }],
			['locating an empty path, which names nothing,', { type: 'locateLocal', path: '' }],
			['ending a watch that is not decimal', { type: 'unwatchLocal', watch: 'three' }],
			['a rename whose token parts are numbers', { type: 'renameLocal', root: '0', path: 'Users', name: 'a', token: { dev: 1, ino: 2, born: 2 }, newName: 'b' }],
			['a fingerprint with an inode', rename({ kind: 'fingerprint', dev: '4', ino: '2', born: null, fingerprint: '9' })],
			['a fingerprint with a creation time', rename({ kind: 'fingerprint', dev: '4', ino: null, born: '7', fingerprint: '9' })],
			['an identity with a fingerprint', rename({ kind: 'identity', dev: '1', ino: '2', born: null, fingerprint: '9' })],
			['a token of an unknown kind', rename({ kind: 'inode', dev: '1', ino: '2', born: null, fingerprint: null })],
			['a token that does not say its kind', rename({ dev: '1', ino: '2', born: null, fingerprint: null })],
			['a delete that does not say whether it goes to the Trash', { type: 'deleteLocal', root: '0', path: 'Users', name: 'a', token: null }],
			['a delete without a token', { type: 'deleteLocal', root: '0', path: 'Users', name: 'a' }],
		]);
	});
	suite('sharing policy', () => {
		accepts(isEngineCommand, [
			['a sharing policy query', { type: 'sharingPolicy' }],
			['a complete sharing policy', { type: 'setSharingPolicy', policy: sharingPolicy }],
		]);
		refuses(isEngineCommand, [
			['a sharing policy with a field that is not a boolean', { type: 'setSharingPolicy', policy: { ...sharingPolicy, camera: 'yes' } }],
			['a sharing policy for one device', { type: 'setSharingPolicy', policy: sharingPolicy, endpointId: 'peer' }],
			['a sharing policy with an extra field', { type: 'setSharingPolicy', policy: { ...sharingPolicy, extra: true } }],
		]);
	});
	suite('availability and diagnostics', () => {
		accepts(isEngineCommand, [
			['a device availability query', { type: 'peerAvailability', endpointId: 'peer' }],
			['a hosted services query', { type: 'hostedServices' }],
			['a flight recorder query', { type: 'diagnosticRecorder' }],
			['a log filter query', { type: 'logFilter' }],
			['an engine log tail query', { type: 'engineLogTail' }],
			['a log filter', { type: 'setLogFilter', directives: 'vnilink::files=debug' }],
			['the default log filter', { type: 'setLogFilter', directives: null }],
			['a diagnostic snapshot query', { type: 'diagnosticSnapshot' }],
		]);
		refuses(isEngineCommand, [
			['a device availability query without a device', { type: 'peerAvailability' }],
			['a flight recorder query for one service', { type: 'diagnosticRecorder', service: '4' }],
			['an engine log tail of a named file', { type: 'engineLogTail', path: '/tmp/engine.log' }],
			['a log filter without directives', { type: 'setLogFilter' }],
			['a numeric log filter', { type: 'setLogFilter', directives: 3 }],
			['a log filter longer than 1024', { type: 'setLogFilter', directives: 'x'.repeat(1025) }],
			['a diagnostic snapshot to a named file', { type: 'diagnosticSnapshot', path: '/tmp/x' }],
		]);
	});
	suite('native runtime', () => {
		accepts(isEngineCommand, [
			['a native runtime query', { type: 'nativeRuntime' }],
		]);
		refuses(isEngineCommand, [
			['a computer name with a line break', { type: 'setComputerName', name: 'two\nlines' }],
			['a computer name longer than 128 bytes', { type: 'setComputerName', name: 'é'.repeat(65) }],
			['an empty computer name, which is no name', { type: 'setComputerName', name: '' }],
			['a native runtime query for a device', { type: 'nativeRuntime', endpointId: 'unexpected' }],
			['a camera source named twice', { type: 'setNativeCameraSources', ids: ['one', 'one'] }],
			['an audio source used as microphone and system sound', { type: 'setNativeAudioSources', microphones: ['one'], system: ['one'] }],
			['a permission report with a status for input', { type: 'reportNativePermissions', screen: true, input: 'granted', camera: false, microphone: false }],
		]);
	});
});

suite('isEngineReply', () => {
	ensureNoDisposablesAreLeakedInTestSuite();
	suite('identity, codecs and events', () => {
		accepts(isEngineReply, [
			['an endpoint', { type: 'endpoint', endpointId: 'abc' }],
			['capabilities', { type: 'capabilities', capabilities: { desktop_capture: true, keyboard: true, pointer: true, clipboard: true, file_transfer: true, audio: false, monitors: 1, h264_encode: true, h264_decode: true } }],
			['codecs', { type: 'videoCodecs', codecs }],
			['an unknown host name', { type: 'hostname', hostname: null }],
			['a host name', { type: 'hostname', hostname: 'peer' }],
			['a chosen computer name beside the system\'s', { type: 'computerName', chosen: 'Studio Mac', system: 'studio.local' }],
			['no chosen computer name and no system name', { type: 'computerName', chosen: null, system: null }],
			['an event subscription', { type: 'eventSubscription', subscriptionId: '3' }],
			['no events', { type: 'events', events: [], lagged: '0' }],
			['a known event', { type: 'events', events: [{ type: 'hostingChanged', hosting: true }], lagged: '0' }],
		]);
		refuses(isEngineReply, [
			['a computer name with spaces around it', { type: 'computerName', chosen: ' Studio ', system: null }],
			['a clipboard owner, since no reply names one,', { type: 'ownerSession', session: null }],
			['an unknown event', { type: 'events', events: [{ type: 'bogus' }], lagged: '0' }],
		]);
	});
	suite('services and local files', () => {
		accepts(isEngineReply, [
			['detected shells', shells],
			['detected shells with a chosen one', { ...shells, current: '/bin/bash' }],
			['a terminal policy', { type: 'terminalPolicy', policy: terminalPolicy }],
			['no terminal sessions', { type: 'terminalSessions', sessions: [] }],
			['a terminal session', { type: 'terminalSessions', sessions: [{ id: 'a', process_id: 42, shell: '/bin/zsh', rows: 24, columns: 80, persistent: true, attached: true }] }],
			['places', { type: 'places', places }],
			['a location', { type: 'location', location: { root: '0', path: 'Users/someone', name: 'a.txt' } }],
			['no location for a path outside what is shared', { type: 'location', location: null }],
			['a watch', { type: 'watching', watch: '3' }],
			['a folder page', { type: 'dirPage', page }],
		]);
		refuses(isEngineReply, [
			['a shell that names its program', { ...shells, shells: [{ id: '/bin/zsh', label: 'zsh', program: '/bin/zsh' }] }],
			['an empty automatic shell', { ...shells, automatic: '' }],
			['per-device service permissions', { type: 'servicePermissions', permissions: { desktop: true, files: true, terminal: false, camera: false, system_audio: false, microphone: false, recording: false, system_actions: false } }],
			['a location with an absolute path', { type: 'location', location: { root: '0', path: '/Users', name: 'a.txt' } }],
			['places with an absolute folder', { type: 'places', places: { ...places, folders: [{ kind: 'home', root: '2', path: '/Users' }] } }],
			['a folder page with a parent entry', { type: 'dirPage', page: { ...page, entries: [{ ...entry, name: '..' }] } }],
		]);
	});
	suite('pairing and sharing', () => {
		accepts(isEngineReply, [
			['a pairing preview with the largest ids as decimal strings', { type: 'pairingPreview', preview }],
			['a sharing policy', { type: 'sharingPolicy', policy: sharingPolicy }],
		]);
		refuses(isEngineReply, [
			['a pairing preview with a numeric id', { type: 'pairingPreview', preview: { ...preview, id: Number(preview.id) } }],
			['a device with only an id', { type: 'device', device: { endpoint_id: 'peer' } }],
		]);
	});
	suite('availability', () => {
		accepts(isEngineReply, [
			['no availability report', { type: 'peerAvailability', report: null }],
			['an availability report', { type: 'peerAvailability', report: [{ capability: 'camera', status: 'turnedOff' }, { capability: 'files', status: 'available' }] }],
			['no hosted services', { type: 'services', services: [] }],
		]);
		refuses(isEngineReply, [
			['an availability report with an unknown status', { type: 'peerAvailability', report: [{ capability: 'camera', status: 'off' }] }],
			['an availability report with an unknown capability', { type: 'peerAvailability', report: [{ capability: 'webcam', status: 'available' }] }],
			['an availability report naming a capability twice', { type: 'peerAvailability', report: [{ capability: 'camera', status: 'available' }, { capability: 'camera', status: 'turnedOff' }] }],
			['a hosted service with only an id', { type: 'services', services: [{ id: '1' }] }],
		]);
	});
	suite('diagnostics', () => {
		accepts(isEngineReply, [
			['a flight recorder of JSON lines', { type: 'diagnosticRecorder', events: recorderLine }],
			['an empty flight recorder', { type: 'diagnosticRecorder', events: '' }],
			['a log filter from settings', { type: 'logFilter', directives: 'off', source: 'setting' }],
			['an engine log tail', { type: 'engineLogTail', path: '~/engine.log', text: '{"ms":1}\n' }],
			['an engine log tail without a file', { type: 'engineLogTail', path: null, text: '' }],
			['a diagnostic snapshot', { type: 'diagnosticSnapshot', snapshot: diagnosticSnapshot }],
		]);
		refuses(isEngineReply, [
			['a flight recorder line that is not JSON', { type: 'diagnosticRecorder', events: 'not json\n' }],
			['a flight recorder line that is not an object', { type: 'diagnosticRecorder', events: '[1]\n' }],
			['a flight recorder with an extra field', { type: 'diagnosticRecorder', events: recorderLine, extra: 1 }],
			['a flight recorder above its bound', { type: 'diagnosticRecorder', events: `${JSON.stringify({ pad: 'x'.repeat(4 * 1024 * 1024) })}\n` }],
			['a log filter from an unknown source', { type: 'logFilter', directives: 'off', source: 'file' }],
			['a log filter with an extra field', { type: 'logFilter', directives: 'off', source: 'default', extra: 1 }],
			['an engine log tail without its path', { type: 'engineLogTail', text: '' }],
			['an engine log tail longer than 256 KiB', { type: 'engineLogTail', path: null, text: 'x'.repeat(256 * 1024 + 1) }],
			['a diagnostic snapshot with an extra field', { type: 'diagnosticSnapshot', snapshot: diagnosticSnapshot, extra: 1 }],
			['an empty diagnostic snapshot', { type: 'diagnosticSnapshot', snapshot: '' }],
			['a diagnostic snapshot that is not JSON', { type: 'diagnosticSnapshot', snapshot: '{not json' }],
			['a diagnostic snapshot that is not an object', { type: 'diagnosticSnapshot', snapshot: '[4]' }],
			['a diagnostic snapshot without its version', { type: 'diagnosticSnapshot', snapshot: JSON.stringify({ pipelines: [] }) }],
			// The cap counts bytes: 64 KiB of text is the most the engine exports.
			['a diagnostic snapshot above 64 KiB in bytes but not in characters', { type: 'diagnosticSnapshot', snapshot: JSON.stringify({ version: 4, pad: 'é'.repeat(33 * 1024) }) }],
		]);
	});
	suite('native runtime', () => {
		accepts(isEngineReply, [
			['a native runtime', runtime],
		]);
		refuses(isEngineReply, [
			['a native runtime naming a capability twice', { ...runtime, capabilities: runtime.capabilities.map(() => runtime.capabilities[0]) }],
			['a device of an unknown kind', { type: 'nativeDevices', devices: [{ id: 'one', name: 'Camera', kind: 'unknown' }] }],
		]);
	});
});

suite('isEngineEvent', () => {
	ensureNoDisposablesAreLeakedInTestSuite();
	const failed = (closed: object) => ({ type: 'serviceChanged', service: { ...service, phase: { Closed: closed } } });
	accepts(isEngineEvent, [
		['a hosting change', { type: 'hostingChanged', hosting: false }],
		['a folder change with a decimal watch id', { type: 'folderChanged', watch: '4' }],
		['a paired device', { type: 'devicePaired', endpoint_id: 'peer' }],
		['a control request', { type: 'controlRequested', endpoint_id: 'peer' }],
		['a control check of six digits', { type: 'controlCheck', endpoint_id: 'peer', check: '123456' }],
		['a renamed device', { type: 'peerRenamed', endpoint_id: 'peer', hostname: null }],
		['an active session', { type: 'sessionChanged', id: '7', generation: '9', role: 'Host', phase: 'Active', peer_id: 'peer' }],
		['a failed session', { type: 'sessionChanged', id: '7', generation: '9', role: 'Host', phase: { Failed: { message: 'lost' } }, peer_id: 'peer' }],
		['a service change', { type: 'serviceChanged', service }],
		['a revoked service', failed({ cause: 'Revoked' })],
		['a service that failed here, saying what failed,', failed({ cause: 'Failed', failure: 'encoder' })],
		['an input failure', { type: 'inputFailed', id: '1', message: 'denied' }],
		['a topology change', { type: 'topologyChanged', revision: '4' }],
		['a locked host', { type: 'hostLocked', locked: true }],
		['a presence change', { type: 'presenceChanged', endpoint_id: 'peer', presence: 'notAccepting' }],
		['an availability change', { type: 'peerAvailabilityChanged', endpoint_id: 'peer' }],
	]);
	refuses(isEngineEvent, [
		['a numeric watch id', { type: 'folderChanged', watch: 4 }],
		['a control request carrying its check', { type: 'controlRequested', endpoint_id: 'peer', check: '123456' }],
		['a control check that is not six digits', { type: 'controlCheck', endpoint_id: 'peer', check: '123' }],
		['a numeric session id', { type: 'sessionChanged', id: 7, generation: '9', role: 'Host', phase: 'Active', peer_id: 'peer' }],
		['an unknown service kind', { type: 'serviceChanged', service: { ...service, kind: 'Hologram' } }],
		['an unknown closing cause', failed({ cause: 'Abducted' })],
		['a failed service that does not say what failed', failed({ cause: 'Failed' })],
		['a failed service with an unknown failure', failed({ cause: 'Failed', failure: 'boredom' })],
		['a failure on a service that did not fail', failed({ cause: 'Revoked', failure: 'encoder' })],
		['a quality update, since a session has no picture quality,', { type: 'qualityUpdated', id: '1', quality: { max_bitrate_kbps: 1000, max_frame_rate: 30, max_width: 1920, max_height: 1080 } }],
		['a keyframe request, since a session has no picture to refresh,', { type: 'keyframeRequested', id: '1' }],
		['a locked host with an extra field', { type: 'hostLocked', locked: true, extra: 1 }],
		['an unknown event', { type: 'nope' }],
		['an unknown presence', { type: 'presenceChanged', endpoint_id: 'peer', presence: 'asleep' }],
		['an availability change with an extra field', { type: 'peerAvailabilityChanged', endpoint_id: 'peer', extra: 1 }],
	]);
});
