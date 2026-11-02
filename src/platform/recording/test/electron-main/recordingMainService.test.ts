// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'assert';
import { mkdtemp, readdir, readFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { VSBuffer } from '../../../../base/common/buffer.js';
import { DisposableStore } from '../../../../base/common/lifecycle.js';
import { ensureNoDisposablesAreLeakedInTestSuite } from '../../../../base/test/common/utils.js';
import { IConfigurationMainService } from '../../../configuration/electron-main/configurationMainService.js';
import { TestInstantiationService } from '../../../instantiation/test/common/instantiationServiceMock.js';
import { RecordingMainService } from '../../electron-main/recordingMainService.js';

const png = VSBuffer.wrap(Uint8Array.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 1, 2, 3]));

const name = 'Studio 2026-10-19 14.32.05.png';

/** Arguments arrive from the window unchecked; these are refused. */
const refusedPictures: readonly (readonly [string, unknown, unknown])[] = [
	['a name that leaves the folder', '../escape.png', png],
	['a name with a folder in it', 'nested/picture.png', png],
	['bytes that are not a PNG', 'picture.png', VSBuffer.fromString('not a picture')],
	['bytes that are not a buffer', 'picture.png', Uint8Array.from(png.buffer)],
];

suite('RecordingMainService', () => {
	const disposables = new DisposableStore();
	let folder: string;

	/** The service, with `saved` as the saved recording folder and /Pictures as the operating system's. */
	function createService(saved: string): RecordingMainService {
		const instantiationService = disposables.add(new TestInstantiationService());
		instantiationService.stub(IConfigurationMainService, { getValue: (key: string) => key === 'recording.directory' ? saved : undefined });
		return new RecordingMainService(instantiationService.get(IConfigurationMainService), () => '/Pictures');
	}
	setup(async () => {
		folder = await mkdtemp(join(tmpdir(), 'vnilink-pictures-'));
	});
	teardown(async () => {
		disposables.clear();
		await rm(folder, { recursive: true, force: true });
	});
	test('the default folder is VniLink in Pictures, whatever folder is saved', async () => {
		assert.strictEqual(await createService('/custom-recordings').getDefaultDirectory(), join('/Pictures', 'VniLink'));
	});
	test('a picture lands in the recordings folder under its own name', async () => {
		await createService(folder).savePicture(name, png);
		assert.deepStrictEqual([...await readFile(join(folder, name))], [...png.buffer]);
	});
	test('a picture with the name of another is numbered, never written over it', async () => {
		const recording = createService(folder);
		await recording.savePicture(name, png);
		await recording.savePicture(name, png);
		assert.deepStrictEqual((await readdir(folder)).sort(), ['Studio 2026-10-19 14.32.05 (2).png', name]);
		assert.deepStrictEqual([...await readFile(join(folder, 'Studio 2026-10-19 14.32.05 (2).png'))], [...png.buffer]);
	});
	for (const [description, pictureName, bytes] of refusedPictures) {
		test(`a picture is refused for ${description}, and nothing is written`, async () => {
			await assert.rejects(createService(folder).savePicture(pictureName, bytes), /Invalid picture/);
			assert.deepStrictEqual(await readdir(folder), []);
		});
	}
	ensureNoDisposablesAreLeakedInTestSuite();
});
