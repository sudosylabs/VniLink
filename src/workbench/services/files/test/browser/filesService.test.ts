// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'assert';
import { DisposableStore } from '../../../../../base/common/lifecycle.js';
import { ensureNoDisposablesAreLeakedInTestSuite } from '../../../../../base/test/common/utils.js';
import { IDataChannelService } from '../../../../../platform/engine/common/dataChannelService.js';
import { engineErrorCategory, type RemoteStatus } from '../../../../../platform/engine/common/engine.js';
import { IEngineService } from '../../../../../platform/engine/common/engineService.js';
import type { FileAnswer, FileEntry, FileJob, FileOperation, FilePage } from '../../../../../platform/engine/common/files.js';
import { TestDataChannelService } from '../../../../../platform/engine/test/common/testDataChannelService.js';
import { TestEngineService } from '../../../../../platform/engine/test/common/testEngineService.js';
import { TestInstantiationService } from '../../../../../platform/instantiation/test/common/instantiationServiceMock.js';
import { ILogService, NullLogService } from '../../../../../platform/log/common/log.js';
import type { FolderLocation } from '../../../../../platform/state/common/uiState.js';
import { IDeviceService } from '../../../devices/deviceService.js';
import type { FilesFailure, IFilesSession } from '../../filesService.js';
import { filesOpenFailure, filesRefusal, FilesService } from '../../filesServiceImpl.js';

/** A transfer of `a.txt`, 40 of its 100 bytes sent. */
function job(id: string, state: FileJob['state']): FileJob {
	return {
		id, peer: 'p', label: 'a.txt', direction: 'Upload', sender: true, state, total_bytes: '100', done_bytes: '40', files_total: '1', files_done: '0',
		resumed_files: '0', skipped_files: '0', skipped_symlinks: '0', waiting_file: null, current_file: null, error: null, destination: null,
	};
}

/** The other computer's jobs packet, as its channel sends one unasked. */
function jobsPacket(sequence: number, jobs: readonly FileJob[]) {
	return { type: 'data' as const, sequence, kind: 'jobs', metadata: JSON.stringify({ jobs }) };
}

const folder: FolderLocation = { root: '0', path: 'Users/someone' };

/** An answer to every operation that names nothing: no jobs, nothing else. */
function quiet(operation: FileOperation): FileAnswer {
	return operation.type === 'jobs' ? { ok: [] } : { ok: null };
}

suite('FilesService', () => {
	const disposables = new DisposableStore();
	let dataChannels: TestDataChannelService;
	let status: RemoteStatus;
	setup(() => {
		dataChannels = new TestDataChannelService();
		dataChannels.fileAnswer = quiet;
		status = 'available';
	});
	teardown(() => {
		// A session still connected ends as one whose channel was lost, which releases it.
		for (const channel of dataChannels.opened) {
			if (!channel.closed) {
				channel.onError(new Error('Data channel disconnected'));
			}
		}
		disposables.clear();
	});

	/** The service, as the workbench builds it, on a computer connected to `peer` as session 4 of generation 2. */
	function createService(): FilesService {
		const engine = disposables.add(new TestEngineService(command => {
			assert.strictEqual(command.type, 'controllerSessions');
			return { type: 'sessions', sessions: [{ id: '4', generation: '2', role: 'Controller', peer_id: 'peer', phase: 'Active' }] };
		}));
		const instantiationService = disposables.add(new TestInstantiationService());
		instantiationService.stub(IEngineService, engine);
		instantiationService.stub(IDeviceService, {
			availability: (id: string) => {
				assert.strictEqual(id, 'peer');
				return Promise.resolve([{ capability: 'files', status }]);
			},
		});
		instantiationService.stub(ILogService, disposables.add(new NullLogService()));
		instantiationService.stub(IDataChannelService, dataChannels);
		return instantiationService.createInstance(FilesService);
	}

	/** Files on `peer`, open in one tab. */
	async function openSession(service = createService()): Promise<IFilesSession> {
		const result = await service.open('peer');
		assert.ok('session' in result, 'Files did not open');
		return result.session;
	}

	/** The operations of `type` the other computer was asked, oldest first. */
	function asked<T extends FileOperation['type']>(type: T): Extract<FileOperation, { type: T }>[] {
		return dataChannels.opened
			.flatMap(channel => channel.fileOperations)
			.filter((operation): operation is Extract<FileOperation, { type: T }> => operation.type === type);
	}
	test('Files turned off on the other computer says so, and no channel opens', async () => {
		status = 'turnedOff';
		assert.deepStrictEqual(await createService().open('peer'), { failure: 'turnedOff' });
		assert.deepStrictEqual(dataChannels.requests, []);
	});
	test('Files that needs permission on the other computer is refused as such', () => {
		assert.strictEqual(filesRefusal('needsPermission'), 'needsPermission');
	});
	const openFailures: readonly (readonly [string, FilesFailure])[] = [
		['TransferDenied: off', 'turnedOff'],
		['StaleSession: gone', 'disconnected'],
	];
	for (const [message, failure] of openFailures) {
		test(`a channel refused with "${message}" reports ${failure}`, () => {
			assert.strictEqual(filesOpenFailure(new Error(message)), failure);
		});
	}
	test('the channel opens for Files on the active connection', async () => {
		const session = await openSession();
		assert.deepStrictEqual(dataChannels.requests, [{ kind: 'files', sessionId: '4', generation: '2' }]);
		session.release();
	});
	test('two tabs opening one computer at once share its session and its channel', async () => {
		const service = createService();
		const [first, second] = await Promise.all([service.open('peer'), service.open('peer')]);
		assert.ok('session' in first && 'session' in second);
		assert.strictEqual(first.session, second.session);
		assert.strictEqual(dataChannels.opened.length, 1);
		first.session.release();
		second.session.release();
	});
	test('a new connection first carries on with what a lost one left, then reads the jobs', async () => {
		const session = await openSession();
		assert.deepStrictEqual(dataChannels.opened[0].fileOperations.map(operation => operation.type), ['adopt', 'jobs']);
		session.release();
	});
	test('a new session holds the jobs the other computer reports', async () => {
		dataChannels.fileAnswer = operation => operation.type === 'jobs' ? { ok: [job('3', 'Complete')] } : { ok: null };
		const session = await openSession();
		assert.deepStrictEqual(session.jobs.map(item => item.id), ['3']);
		session.release();
	});
	test('a jobs packet updates the session\'s jobs and reports the change once', async () => {
		dataChannels.fileAnswer = operation => operation.type === 'jobs' ? { ok: [job('3', 'Complete')] } : { ok: null };
		const session = await openSession();
		let changes = 0;
		disposables.add(session.onDidChangeJobs(() => changes++));
		await dataChannels.opened[0].consume(jobsPacket(1, [job('5', 'Transferring')]));
		assert.strictEqual(changes, 1);
		assert.deepStrictEqual(session.jobs.map(item => [item.id, item.state]), [['3', 'Complete'], ['5', 'Transferring']]);
		session.release();
	});
	test('a running transfer keeps the connection after every tab lets go', async () => {
		const service = createService();
		const session = await openSession(service);
		await openSession(service);
		await dataChannels.opened[0].consume(jobsPacket(1, [job('5', 'Transferring')]));
		session.release();
		session.release();
		assert.strictEqual(dataChannels.opened[0].closed, false);
		assert.strictEqual(session.ended, false);
	});
	test('the connection closes once its last transfer finishes with no tab holding it', async () => {
		const session = await openSession();
		const [channel] = dataChannels.opened;
		await channel.consume(jobsPacket(1, [job('5', 'Transferring')]));
		session.release();
		await channel.consume(jobsPacket(2, [job('5', 'Complete')]));
		assert.strictEqual(channel.closed, true, 'an idle connection stayed open');
		assert.strictEqual(session.ended, true);
	});
	test('a refusal from the other computer rejects with its category, and the connection lives', async () => {
		dataChannels.fileAnswer = operation => operation.type === 'rename' ? { error: 'FileExists: the name is taken' } : quiet(operation);
		const session = await openSession();
		const entry: FileEntry = { name: 'a.txt', kind: 'file', size: '1', mtimeMs: '0', token: { kind: 'identity', dev: '1', ino: '2', born: '7', fingerprint: null } };
		const error = await session.remote.rename(folder, entry, 'b.txt').then(() => undefined, (failure: unknown) => failure);
		assert.strictEqual(engineErrorCategory(error), 'FileExists');
		assert.strictEqual(session.ended, false);
		session.release();
	});
	test('a selection is one transfer, each item once', async () => {
		const session = await openSession();
		await session.copy('local', [{ folder, name: 'a.txt' }, { folder, name: 'Photos' }, { folder, name: 'a.txt' }], { root: '2', path: 'Users/someone/Downloads' });
		assert.deepStrictEqual(asked('submit').map(operation => [operation.direction, operation.srcPath, operation.names, operation.dstPath]), [
			['upload', 'Users/someone', ['a.txt', 'Photos'], 'Users/someone/Downloads'],
		]);
		session.release();
	});
	test('a copy from the other computer is a download into this computer\'s folder', async () => {
		const session = await openSession();
		await session.copy('remote', [{ folder: { root: '2', path: '' }, name: 'x' }], folder);
		assert.deepStrictEqual(asked('submit'), [{ type: 'submit', direction: 'download', srcRoot: '2', srcPath: '', names: ['x'], dstRoot: '0', dstPath: 'Users/someone' }]);
		session.release();
	});
	test('items dropped from two folders are a transfer each', async () => {
		const session = await openSession();
		await session.copy('local', [{ folder, name: 'a.txt' }, { folder: { root: '0', path: 'Users/someone/Desktop' }, name: 'b.txt' }, { folder, name: 'c.txt' }], { root: '2', path: '' });
		assert.deepStrictEqual(asked('submit').map(operation => [operation.srcPath, operation.names]), [['Users/someone', ['a.txt', 'c.txt']], ['Users/someone/Desktop', ['b.txt']]]);
		session.release();
	});
	test('a folder listing reads every page', async () => {
		const token = { kind: 'identity', dev: null, ino: null, born: null, fingerprint: null } as const;
		const pages: Record<string, FilePage> = {
			'0': { entries: [{ name: 'a', kind: 'dir', size: '0', mtimeMs: '0', token }], total: '2', nextOffset: '1', truncated: false, skipped: '0' },
			'1': { entries: [{ name: 'b', kind: 'file', size: '9', mtimeMs: '0', token }], total: '2', nextOffset: null, truncated: false, skipped: '1' },
		};
		dataChannels.fileAnswer = operation => operation.type === 'list' ? { ok: pages[operation.offset] } : quiet(operation);
		const session = await openSession();
		const listing = await session.remote.list({ root: '0', path: '' });
		assert.deepStrictEqual(listing.entries.map(entry => entry.name), ['a', 'b']);
		assert.strictEqual(listing.skipped, 1);
		session.release();
	});
	ensureNoDisposablesAreLeakedInTestSuite();
});
