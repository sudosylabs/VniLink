// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'assert';
import { ensureNoDisposablesAreLeakedInTestSuite } from '../../../../base/test/common/utils.js';
import { channelAdmission, commandAdmission } from '../../common/admission.js';
import type { DataChannelRequest } from '../../common/dataChannel.js';

const on = { sessionId: '4', generation: '2' };

function channel(kind: DataChannelRequest['kind'], media?: DataChannelRequest['media']): DataChannelRequest {
	return { requestId: 'r', kind, ...on, media };
}

suite('Admission', () => {
	ensureNoDisposablesAreLeakedInTestSuite();
	test('each channel asks for the service it uses', () => {
		const services = (['video', 'input', 'files', 'terminal'] as const).map(kind => channelAdmission(channel(kind))?.service);
		assert.deepStrictEqual(services, ['Desktop', 'Desktop', 'Files', 'Terminal']);
		assert.deepStrictEqual(channelAdmission(channel('camera', { type: 'camera', deviceId: 'c', width: 640, height: 480, fps: 30 })), { service: 'Camera', ...on });
		assert.strictEqual(channelAdmission(channel('audio', { type: 'audio', deviceId: 'm', sourceKind: 'Microphone', channels: 1, voice: true }))?.service, 'Microphone');
		assert.strictEqual(channelAdmission(channel('audio', { type: 'audio', deviceId: 's', sourceKind: 'System', channels: 2, voice: false }))?.service, 'SystemAudio');
	});
	test('a screenshot asks nothing', () => {
		assert.strictEqual(channelAdmission(channel('snapshot', { type: 'snapshot', prompt: false, access: false })), undefined);
	});
	test('commands that begin a use ask, on the connection they name', () => {
		assert.deepStrictEqual([
			commandAdmission({ type: 'remoteCameraDevices', ...on }),
			commandAdmission({ type: 'remoteAudioDevices', ...on, kind: 'System' }),
			commandAdmission({ type: 'remoteTerminalSessions', connectionId: '4', generation: '2' }),
			commandAdmission({ type: 'systemAction', ...on, action: 'lock' }),
			commandAdmission({ type: 'setSystemPrivacy', ...on, enabled: true }),
		].map(admission => admission && `${admission.service} ${admission.sessionId}/${admission.generation}`), [
			'Camera 4/2', 'SystemAudio 4/2', 'Terminal 4/2', 'System 4/2', 'System 4/2',
		]);
	});
	test('following up on, cancelling or closing what was admitted asks nothing', () => {
		for (const command of [
			{ type: 'systemOperation', ...on, id: '7' },
			{ type: 'cancelSystemAction', ...on, id: '7' },
			{ type: 'closeRemoteTerminalSession', connectionId: '4', generation: '2', id: 'a' },
			{ type: 'snapshot' },
		]) {
			assert.strictEqual(commandAdmission(command), undefined, command.type);
		}
	});
});
