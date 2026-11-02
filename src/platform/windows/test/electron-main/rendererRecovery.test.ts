// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'assert';
import type { RenderProcessGoneDetails } from 'electron';
import { ensureNoDisposablesAreLeakedInTestSuite } from '../../../../base/test/common/utils.js';
import { NullLogService } from '../../../log/common/log.js';
import { recoverRenderer, type RecoverableWindow } from '../../electron-main/rendererRecovery.js';

type GoneListener = (event: unknown, details: RenderProcessGoneDetails) => void;

/** A window whose renderer a test ends or stalls, counting the reloads asked of it. */
class TestWindow implements RecoverableWindow {
	public reloads = 0;
	public destroyed = false;
	private readonly goneListeners: GoneListener[] = [];
	private readonly unresponsiveListeners: (() => void)[] = [];
	public readonly webContents = {
		on: (_event: 'render-process-gone', listener: GoneListener): void => {
			this.goneListeners.push(listener);
		},
		reload: (): void => {
			this.reloads++;
		},
	};

	public on(event: 'unresponsive' | 'responsive', listener: () => void): void {
		if (event === 'unresponsive') {
			this.unresponsiveListeners.push(listener);
		}
	}

	public isDestroyed(): boolean {
		return this.destroyed;
	}

	public renderProcessGone(reason: RenderProcessGoneDetails['reason'], exitCode: number): void {
		for (const listener of this.goneListeners) {
			listener({}, { reason, exitCode });
		}
	}

	public stopResponding(): void {
		for (const listener of this.unresponsiveListeners) {
			listener();
		}
	}
}

class RecordingLogService extends NullLogService {
	public readonly errors: string[] = [];
	public readonly warnings: string[] = [];

	public override error(message: string | Error): void {
		this.errors.push(String(message));
	}

	public override warn(message: string): void {
		this.warnings.push(message);
	}
}

suite('recoverRenderer', () => {
	const disposables = ensureNoDisposablesAreLeakedInTestSuite();
	let window: TestWindow;
	let log: RecordingLogService;
	let time: number;
	setup(() => {
		window = new TestWindow();
		log = disposables.add(new RecordingLogService());
		time = 0;
		recoverRenderer(window, log, () => time);
	});
	test('a crashed renderer is reloaded', () => {
		window.renderProcessGone('crashed', 11);
		assert.strictEqual(window.reloads, 1);
	});
	test('a renderer that dies again right after a reload is not reloaded again', () => {
		window.renderProcessGone('crashed', 11);
		time = 5_000;
		window.renderProcessGone('oom', 1);
		assert.strictEqual(window.reloads, 1);
		assert.strictEqual(log.errors.at(-1), '[window] the renderer died again right after a reload; it is not reloaded again');
	});
	test('a renderer that dies long after a reload is reloaded again', () => {
		window.renderProcessGone('crashed', 11);
		time = 60_000;
		window.renderProcessGone('killed', 9);
		assert.strictEqual(window.reloads, 2);
	});
	test('a renderer that exits cleanly is left alone', () => {
		window.renderProcessGone('clean-exit', 0);
		assert.strictEqual(window.reloads, 0);
	});
	test('a destroyed window is not reloaded', () => {
		window.destroyed = true;
		window.renderProcessGone('crashed', 11);
		assert.strictEqual(window.reloads, 0);
	});
	test('every renderer exit is logged with its reason and exit code', () => {
		window.renderProcessGone('crashed', 11);
		window.renderProcessGone('clean-exit', 0);
		assert.deepStrictEqual(log.errors, ['[window] renderer gone: crashed (exit code 11)', '[window] renderer gone: clean-exit (exit code 0)']);
	});
	test('a renderer that stops responding is logged as a warning', () => {
		window.stopResponding();
		assert.deepStrictEqual(log.warnings, ['[window] the renderer stopped responding']);
	});
});
