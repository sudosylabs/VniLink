// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'assert';
import { ensureNoDisposablesAreLeakedInTestSuite } from '../../../../../base/test/common/utils.js';
import { IDataChannelService } from '../../../../../platform/engine/common/dataChannelService.js';
import { IEngineService } from '../../../../../platform/engine/common/engineService.js';
import { TestDataChannelService } from '../../../../../platform/engine/test/common/testDataChannelService.js';
import { TestEngineService } from '../../../../../platform/engine/test/common/testEngineService.js';
import { TestInstantiationService } from '../../../../../platform/instantiation/test/common/instantiationServiceMock.js';
import { ScreenSnapshotService } from '../../screenSnapshotServiceImpl.js';

suite('ScreenSnapshotService', () => {
	const disposables = ensureNoDisposablesAreLeakedInTestSuite();
	function service(refuse: Error | undefined): ScreenSnapshotService {
		const dataChannels = new TestDataChannelService();
		dataChannels.refuse = () => refuse;
		const engine = disposables.add(new TestEngineService(command => {
			assert.strictEqual(command.type, 'controllerSessions');
			return { type: 'sessions', sessions: [{ id: '4', generation: '2', role: 'Controller', peer_id: 'peer', phase: 'Active' }] };
		}));
		const instantiation = disposables.add(new TestInstantiationService());
		instantiation.stub(IEngineService, engine);
		instantiation.stub(IDataChannelService, dataChannels);
		return disposables.add(instantiation.createInstance(ScreenSnapshotService));
	}
	test('a host that has not admitted this computer says so, and does not call it a failed capture', async () => {
		const result = await service(new Error('NotAdmitted: not admitted')).capture('peer', new AbortController().signal, false);
		assert.deepStrictEqual(result, { status: 'notAdmitted' });
	});
	test('any other refusal stays a failed capture', async () => {
		const result = await service(new Error('Busy: the host is busy')).capture('peer', new AbortController().signal, false);
		assert.deepStrictEqual(result, { status: 'failed' });
	});
});
