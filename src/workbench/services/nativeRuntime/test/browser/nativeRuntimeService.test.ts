// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'assert';
import { ensureNoDisposablesAreLeakedInTestSuite } from '../../../../../base/test/common/utils.js';
import { IConfigurationService } from '../../../../../platform/configuration/common/configuration.js';
import type { RecordingStatus } from '../../../../../platform/engine/common/engine.js';
import { IEngineService } from '../../../../../platform/engine/common/engineService.js';
import { TestEngineService } from '../../../../../platform/engine/test/common/testEngineService.js';
import { TestInstantiationService } from '../../../../../platform/instantiation/test/common/instantiationServiceMock.js';
import { INativeHostService } from '../../../../../platform/native/common/native.js';
import { IRecordingService } from '../../../../../platform/recording/common/recording.js';
import { NativeRuntimeService } from '../../nativeRuntimeServiceImpl.js';

const gibibyte = 1073741824;

/** Settings as the configuration service holds them, read by key. */
class TestSettings implements Pick<IConfigurationService, 'getValue'> {
	constructor(private readonly values: Record<string, unknown>) { }

	// The type parameter mirrors VS Code's IConfigurationService.getValue contract.
	// eslint-disable-next-line @typescript-eslint/no-unnecessary-type-parameters
	public getValue<T>(section?: unknown): T {
		return this.values[String(section)] as T;
	}
}

const status: RecordingStatus = { id: '4', phase: 'Recording', failure: null, frames: '0', bytes: '0', durationMs: '0', segments: 1 };

suite('NativeRuntimeService', () => {
	const disposables = ensureNoDisposablesAreLeakedInTestSuite();
	let engine: TestEngineService;
	/** How many new recording files the service asked main for. */
	let destinations: number;

	/**
	 * The service, with recording settings as the configuration service holds
	 * them: sizes in GiB, durations in minutes.
	 */
	function createService(settings: Record<string, unknown> = {}): NativeRuntimeService {
		const values: Record<string, unknown> = { 'recording.directory': '', 'recording.maxSegmentSize': 1, 'recording.maxSegmentDuration': 1, 'recording.maxSegments': 1, 'recording.maxTotalSize': 1, ...settings };
		const instantiationService = disposables.add(new TestInstantiationService());
		instantiationService.stub(IEngineService, engine);
		instantiationService.stub(INativeHostService, {});
		instantiationService.stub(IRecordingService, {
			createDestination: () => {
				destinations++;
				return Promise.resolve('/Pictures/VniLink/new.mkv');
			},
		});
		instantiationService.stub(IConfigurationService, new TestSettings(values));
		return instantiationService.createInstance(NativeRuntimeService);
	}
	setup(() => {
		engine = disposables.add(new TestEngineService(() => ({ type: 'recordingStatus', status })));
		destinations = 0;
	});
	test('a recording takes its limits from the persisted settings', async () => {
		const service = createService({ 'recording.maxSegmentSize': 2, 'recording.maxSegmentDuration': 1, 'recording.maxSegments': 2, 'recording.maxTotalSize': 4 });
		await service.startRecording({ id: '1', generation: '7' }, { destination: '/chosen/session.mkv' });
		assert.deepStrictEqual(engine.commands, [{
			type: 'startRecording', sessionId: '1', generation: '7', destination: '/chosen/session.mkv',
			maxBytes: String(2 * gibibyte), maxDurationMs: '60000', retention: { maxSegments: 2, maxTotalBytes: String(4 * gibibyte) },
		}]);
	});
	test('a recording keeps the destination it was given, without asking main for one', async () => {
		await createService().startRecording({ id: '1', generation: '7' }, { destination: '/another.mkv' });
		assert.strictEqual(destinations, 0);
	});
	test('a recording\'s own override replaces only that setting', async () => {
		await createService({ 'recording.maxSegmentSize': 2 }).startRecording({ id: '1', generation: '7' }, { destination: '/another.mkv', maxDurationMs: '120000' });
		const [command] = engine.commands;
		assert.ok(command.type === 'startRecording');
		assert.strictEqual(command.maxDurationMs, '120000');
		assert.strictEqual(command.maxBytes, String(2 * gibibyte));
	});
	test('a recording without a destination is given a new file in the default folder by main', async () => {
		await createService().startRecording({ id: '1', generation: '2' }, {});
		assert.strictEqual(destinations, 1);
		const [command] = engine.commands;
		assert.ok(command.type === 'startRecording');
		assert.strictEqual(command.destination, '/Pictures/VniLink/new.mkv');
	});
});
