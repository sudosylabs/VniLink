// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'assert';
import { $ } from '../../../../../base/browser/dom.js';
import { DeferredPromise, timeout } from '../../../../../base/common/async.js';
import { ensureNoDisposablesAreLeakedInTestSuite } from '../../../../../base/test/common/utils.js';
import { TestInstantiationService } from '../../../../../platform/instantiation/test/common/instantiationServiceMock.js';
import type { WorkbenchTab } from '../../tabService.js';
import { TabService } from '../../tabServiceImpl.js';

/** A tab that counts its disposals and asks `approve`, when given, before it closes. */
class TestTab implements WorkbenchTab {
	public readonly label: string;
	public readonly element = $('div');
	public disposals = 0;
	public approvals = 0;

	constructor(
		public readonly id: string,
		public readonly permanent = false,
		private readonly approve?: () => Promise<boolean>,
	) {
		this.label = id;
	}

	public canClose(): boolean | Promise<boolean> {
		this.approvals++;
		return this.approve?.() ?? true;
	}

	public dispose(): void {
		this.disposals++;
	}
}

suite('TabService', () => {
	const disposables = ensureNoDisposablesAreLeakedInTestSuite();
	function createService(): TabService {
		const instantiationService = disposables.add(new TestInstantiationService());
		return disposables.add(instantiationService.createInstance(TabService));
	}
	const ids = (service: TabService): string[] => service.tabs.map(tab => tab.id);
	test('selecting another tab keeps the inactive one open', () => {
		const service = createService();
		const first = new TestTab('first');
		const second = new TestTab('second');
		service.open(first);
		service.open(second);
		service.select('first');
		assert.strictEqual(service.activeTab, first);
		assert.strictEqual(second.disposals, 0);
	});
	test('opening a tab whose id is already open disposes the duplicate and keeps one tab', () => {
		const service = createService();
		service.open(new TestTab('first'));
		service.open(new TestTab('second'));
		const duplicate = new TestTab('first');
		service.open(duplicate);
		assert.strictEqual(duplicate.disposals, 1);
		assert.deepStrictEqual(ids(service), ['first', 'second']);
	});
	test('moving a tab reorders the list', () => {
		const service = createService();
		service.open(new TestTab('first'));
		service.open(new TestTab('second'));
		service.move('first', 1);
		assert.deepStrictEqual(ids(service), ['second', 'first']);
	});
	test('closing the active tab disposes it and selects its neighbour', async () => {
		const service = createService();
		const first = new TestTab('first');
		const second = new TestTab('second');
		service.open(second);
		service.open(first);
		assert.strictEqual(await service.close('first'), true);
		assert.strictEqual(service.activeTab, second);
		assert.strictEqual(first.disposals, 1);
	});
	test('disposing the service disposes each remaining tab once and a closed tab not again', async () => {
		const service = createService();
		const first = new TestTab('first');
		const second = new TestTab('second');
		service.open(first);
		service.open(second);
		await service.close('first');
		service.dispose();
		service.dispose();
		assert.strictEqual(first.disposals, 1);
		assert.strictEqual(second.disposals, 1);
	});
	suite('close approval', () => {
		let approval: DeferredPromise<boolean>;
		let guarded: TestTab;
		setup(() => {
			approval = new DeferredPromise<boolean>();
			guarded = new TestTab('guarded', false, () => approval.p);
		});
		test('concurrent close requests share one approval', async () => {
			const service = createService();
			service.open(guarded);
			const first = service.close('guarded');
			const second = service.close('guarded');
			assert.strictEqual(first, second);
			await timeout(0);
			assert.strictEqual(guarded.approvals, 1);
			await approval.complete(true);
			assert.strictEqual(await first, true);
		});
		test('a tab that refuses to close stays open', async () => {
			const service = createService();
			service.open(guarded);
			const closing = service.close('guarded');
			await approval.complete(false);
			assert.strictEqual(await closing, false);
			assert.deepStrictEqual(ids(service), ['guarded']);
			assert.strictEqual(guarded.disposals, 0);
		});
		test('an approval that arrives after the service is disposed closes nothing', async () => {
			const service = createService();
			service.open(guarded);
			const closing = service.close('guarded');
			await timeout(0);
			service.dispose();
			await approval.complete(true);
			assert.strictEqual(await closing, false);
			assert.strictEqual(guarded.disposals, 1, 'only the service disposal released the tab');
			assert.deepStrictEqual(service.tabs, []);
		});
	});
	suite('permanent Home', () => {
		let home: TestTab;
		let service: TabService;
		setup(() => {
			home = new TestTab('home', true);
			service = createService();
			service.open(new TestTab('remote'));
			service.open(home);
			service.open(new TestTab('terminal'));
		});
		test('Home is first even when it opens after other tabs', () => {
			assert.deepStrictEqual(ids(service), ['home', 'remote', 'terminal']);
		});
		test('Home cannot be moved and no tab can move before it', () => {
			service.move('home', 2);
			service.move('terminal', 0);
			assert.deepStrictEqual(ids(service), ['home', 'terminal', 'remote']);
		});
		test('Home cannot be closed', async () => {
			assert.strictEqual(await service.close('home'), false);
			assert.strictEqual(home.disposals, 0);
		});
		test('closing every other tab leaves Home selected', async () => {
			await service.close('remote');
			await service.close('terminal');
			assert.strictEqual(service.activeTab, home);
			assert.deepStrictEqual(service.tabs, [home]);
		});
		test('disposing the service disposes Home', () => {
			service.dispose();
			assert.strictEqual(home.disposals, 1);
		});
	});
});
