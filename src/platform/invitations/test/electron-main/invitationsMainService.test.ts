// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'assert';
import { mkdtemp, rm, writeFile } from 'fs/promises';
import { tmpdir } from 'os';
import { join, resolve } from 'path';
import { ensureNoDisposablesAreLeakedInTestSuite } from '../../../../base/test/common/utils.js';
import { NullLogService } from '../../../log/common/log.js';
import { isOpenedInvitations, MAX_INVITATION_BYTES } from '../../common/invitations.js';
import { InvitationsMainService, readInvitation, type InvitationReader } from '../../electron-main/invitationsMainService.js';

suite('InvitationsMainService', () => {
	const store = ensureNoDisposablesAreLeakedInTestSuite();

	/** A service whose reader answers from `files`, recording the paths it was asked for. */
	function service(files: Readonly<Record<string, Uint8Array | undefined>>, asked: string[] = []): InvitationsMainService {
		const reader: InvitationReader = path => {
			asked.push(path);
			return Promise.resolve(files[path]);
		};
		return store.add(new InvitationsMainService(reader, new NullLogService()));
	}
	test('reads only the invitations among a launch\'s arguments, relative to its working directory', async () => {
		const asked: string[] = [];
		const invitations = service({ [resolve('/work', 'Laptop.vnl')]: new Uint8Array([1, 2]) }, asked);
		assert.strictEqual(await invitations.openFromCommandLine(['/app', '--hidden', 'Laptop.vnl', 'notes.txt', '--x.vnl'], '/work'), true);
		assert.deepStrictEqual(asked, [resolve('/work', 'Laptop.vnl')]);
		assert.deepStrictEqual(invitations.take(), [{ name: 'Laptop.vnl', bytes: [1, 2] }]);
	});
	test('names invitations by extension whatever its case', async () => {
		const invitations = service({ '/files/Desk.VNL': new Uint8Array([7]) });
		assert.strictEqual(await invitations.openPaths(['/files/Desk.VNL']), true);
		assert.deepStrictEqual(invitations.take(), [{ name: 'Desk.VNL', bytes: [7] }]);
	});
	test('announces waiting invitations once per opening and hands each out once', async () => {
		const invitations = service({ '/a.vnl': new Uint8Array([1]), '/b.vnl': new Uint8Array([2]) });
		let announced = 0;
		store.add(invitations.onDidOpenInvitations(() => announced++));
		await invitations.openPaths(['/a.vnl', '/b.vnl']);
		assert.strictEqual(announced, 1);
		assert.deepStrictEqual(invitations.take().map(invitation => invitation.name), ['a.vnl', 'b.vnl']);
		assert.deepStrictEqual(invitations.take(), []);
	});
	test('keeps nothing and announces nothing for a file it refuses', async () => {
		const invitations = service({});
		let announced = 0;
		store.add(invitations.onDidOpenInvitations(() => announced++));
		assert.strictEqual(await invitations.openPaths(['/missing.vnl', '/photo.png']), false);
		assert.strictEqual(announced, 0);
		assert.deepStrictEqual(invitations.take(), []);
	});
});

suite('readInvitation', () => {
	ensureNoDisposablesAreLeakedInTestSuite();
	let directory: string;
	setup(async () => {
		directory = await mkdtemp(join(tmpdir(), 'vnilink-invitation-'));
	});
	teardown(async () => {
		await rm(directory, { recursive: true, force: true });
	});
	test('reads a file within the limit', async () => {
		await writeFile(join(directory, 'a.vnl'), new Uint8Array([4, 5, 6]));
		assert.deepStrictEqual(await readInvitation(join(directory, 'a.vnl'), MAX_INVITATION_BYTES), new Uint8Array([4, 5, 6]));
	});
	test('refuses a file larger than the limit, an empty file and a folder', async () => {
		await writeFile(join(directory, 'large.vnl'), new Uint8Array(5));
		await writeFile(join(directory, 'empty.vnl'), new Uint8Array(0));
		assert.strictEqual(await readInvitation(join(directory, 'large.vnl'), 4), undefined);
		assert.strictEqual(await readInvitation(join(directory, 'empty.vnl'), 4), undefined);
		assert.strictEqual(await readInvitation(directory, 4), undefined);
	});
});

suite('isOpenedInvitations', () => {
	ensureNoDisposablesAreLeakedInTestSuite();
	test('accepts names with byte arrays', () => {
		assert.ok(isOpenedInvitations([{ name: 'a.vnl', bytes: [0, 255] }]));
	});
	test('refuses values that are not bytes or are too many', () => {
		assert.ok(!isOpenedInvitations([{ name: 'a.vnl', bytes: [256] }]));
		assert.ok(!isOpenedInvitations([{ name: 'a.vnl', bytes: [] }]));
		assert.ok(!isOpenedInvitations([{ name: 'a.vnl', bytes: new Array<number>(MAX_INVITATION_BYTES + 1).fill(0) }]));
		assert.ok(!isOpenedInvitations([{ bytes: [1] }]));
	});
});
