// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'assert';
import { ensureNoDisposablesAreLeakedInTestSuite } from '../../../../base/test/common/utils.js';
import { NullLogService } from '../../../log/common/log.js';
import type { IStateService } from '../../../state/node/state.js';
import { ApplicationLocationMainService, type ApplicationLocationEnvironment } from '../../electron-main/applicationLocationMainService.js';

/** An {@link IStateService} kept in memory. */
class TestStateService implements IStateService {
	declare public readonly _serviceBrand: undefined;
	public readonly items = new Map<string, unknown>();

	public getItem<T>(key: string, defaultValue?: T): T | undefined {
		return this.items.has(key) ? this.items.get(key) as T : defaultValue;
	}

	public setItem(key: string, data?: object | string | number | boolean | null): void {
		this.items.set(key, data);
	}

	public removeItem(key: string): void {
		this.items.delete(key);
	}

	public close(): Promise<void> {
		return Promise.resolve();
	}
}

/** An application `inApplications` or not, whose person answers `answer` and whose moves succeed unless `moves` is false. */
class TestEnvironment implements ApplicationLocationEnvironment {
	public asked = 0;
	public moved = 0;

	constructor(public readonly applies: boolean, private readonly inApplications: boolean, private readonly answer: boolean, private readonly moves = true) { }

	public isInApplicationsFolder(): boolean {
		return this.inApplications;
	}

	public moveToApplicationsFolder(): boolean {
		this.moved++;
		if (!this.moves) {
			throw new Error('Could not move');
		}
		return true;
	}

	public askToMove(): Promise<boolean> {
		this.asked++;
		return Promise.resolve(this.answer);
	}
}

suite('ApplicationLocationMainService', () => {
	ensureNoDisposablesAreLeakedInTestSuite();

	/** The service over `environment`, with nothing saved. */
	function service(environment: ApplicationLocationEnvironment, state = new TestStateService()): ApplicationLocationMainService {
		return new ApplicationLocationMainService(environment, state, new NullLogService());
	}
	test('a built macOS application outside Applications is movable', () => {
		assert.deepStrictEqual(service(new TestEnvironment(true, false, false)).location, { movable: true });
	});
	test('an application in Applications, or on another platform or from the checkout, is not', () => {
		assert.strictEqual(service(new TestEnvironment(true, true, false)).location.movable, false);
		assert.strictEqual(service(new TestEnvironment(false, false, false)).location.movable, false);
	});
	test('moving when the person agrees reports that the application relaunches', async () => {
		const environment = new TestEnvironment(true, false, true);
		assert.strictEqual(await new ApplicationLocationMainService(environment, new TestStateService(), new NullLogService()).offerAtStartup(), true);
		assert.strictEqual(environment.moved, 1);
	});
	test('a person who keeps it where it is is not asked again at startup', async () => {
		const environment = new TestEnvironment(true, false, false);
		const state = new TestStateService();
		const location = new ApplicationLocationMainService(environment, state, new NullLogService());
		assert.strictEqual(await location.offerAtStartup(), false);
		assert.strictEqual(await location.offerAtStartup(), false);
		assert.deepStrictEqual([environment.asked, environment.moved], [1, 0]);
	});
	test('a declined move can still be made later', () => {
		const environment = new TestEnvironment(true, false, false);
		const state = new TestStateService();
		state.setItem('applicationLocation.moveDeclined', true);
		assert.strictEqual(new ApplicationLocationMainService(environment, state, new NullLogService()).move(), true);
	});
	test('nothing is asked where nothing can move', async () => {
		const environment = new TestEnvironment(true, true, true);
		assert.strictEqual(await new ApplicationLocationMainService(environment, new TestStateService(), new NullLogService()).offerAtStartup(), false);
		assert.strictEqual(environment.asked, 0);
	});
	test('a move that fails leaves the application running where it is', () => {
		const environment = new TestEnvironment(true, false, true, false);
		assert.strictEqual(new ApplicationLocationMainService(environment, new TestStateService(), new NullLogService()).move(), false);
	});
});
