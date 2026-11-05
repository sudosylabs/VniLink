// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'assert';
import { $ } from '../../../../../base/browser/dom.js';
import { Emitter } from '../../../../../base/common/event.js';
import { ensureNoDisposablesAreLeakedInTestSuite } from '../../../../../base/test/common/utils.js';
import { upcastPartial } from '../../../../../base/test/common/mock.js';
import type { INativeHostService, IWindowState } from '../../../../../platform/native/common/native.js';
import type { IMaterialPanel } from '../../../../../platform/window/common/windowMaterial.js';
import { MaterialPanel } from '../../materialPanel.js';

/** Resolves after the next frame, once the panel has had the chance to report. */
function nextFrame(): Promise<void> {
	return new Promise(resolve => {
		requestAnimationFrame(() => {
			setTimeout(resolve, 0);
		});
	});
}

suite('MaterialPanel', () => {
	const disposables = ensureNoDisposablesAreLeakedInTestSuite();
	let element: HTMLElement;
	let reported: (IMaterialPanel | undefined)[];
	let states: Emitter<IWindowState>;
	setup(() => {
		element = document.body.appendChild($('nav'));
		element.style.cssText = 'position: fixed; left: 8px; top: 36px; width: 224px; height: 400px; border-radius: 10px;';
		reported = [];
		states = disposables.add(new Emitter<IWindowState>());
	});
	teardown(() => {
		element.remove();
	});
	function create(): MaterialPanel {
		const nativeHostService = upcastPartial<INativeHostService>({
			onDidChangeWindowState: states.event,
			setMaterialPanel: panel => {
				reported.push(panel);
				return Promise.resolve();
			},
		});
		return disposables.add(new MaterialPanel(element, nativeHostService));
	}
	function zoom(zoomFactor: number): void {
		states.fire({ windowId: 1, revision: 1, focused: true, maximized: false, fullscreen: false, alwaysOnTop: false, zoomFactor, backgroundMaterial: 'glass', accentColor: undefined });
	}
	test('the window learns where the panel is and how round its corners are', async () => {
		create();
		await nextFrame();
		assert.deepStrictEqual(reported, [{ x: 8, y: 36, width: 224, height: 400, cornerRadius: 10 }]);
	});
	test('a panel that grows is reported once more, and one that has not changed is not', async () => {
		create();
		await nextFrame();
		element.style.height = '480px';
		await nextFrame();
		await nextFrame();
		assert.deepStrictEqual(reported.map(panel => panel?.height), [400, 480]);
	});
	test('a hidden panel is withdrawn, and shown again it returns', async () => {
		create();
		await nextFrame();
		element.style.display = 'none';
		await nextFrame();
		element.style.display = '';
		await nextFrame();
		assert.deepStrictEqual(reported.map(panel => panel?.width), [224, undefined, 224]);
	});
	test('a new zoom reports the same panel again, since its points changed', async () => {
		create();
		await nextFrame();
		zoom(1);
		await nextFrame();
		zoom(1);
		zoom(1.25);
		await nextFrame();
		assert.strictEqual(reported.length, 3);
	});
	test('disposing withdraws the panel', async () => {
		const panel = create();
		await nextFrame();
		panel.dispose();
		assert.deepStrictEqual(reported.at(-1), undefined);
		assert.strictEqual(reported.length, 2);
	});
});
