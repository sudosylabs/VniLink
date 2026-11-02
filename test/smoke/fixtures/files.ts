// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import type { IDisposable } from '../../../src/base/common/lifecycle.js';
import { FilesFixture } from '../../../src/workbench/contrib/files/test/browser/filesFixture.js';

/**
 * The Files tab as people see it mid-task, `width` × `height` at most, for the
 * smoke suite's pictures: Downloads on both computers, two items selected
 * here, a transfer running and one waiting, and the list of transfers open.
 */
export async function showFilesFixture(width: number, height: number): Promise<IDisposable> {
	const fixture = new FilesFixture(width, height);
	try {
		await fixture.showMidTask();
	} catch (error) {
		fixture.dispose();
		throw error;
	}
	return fixture;
}
