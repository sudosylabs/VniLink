// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

/** Owns readiness and renderer automation for one real Electron window. */
import { readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import type { BrowserWindow } from 'electron';
import { Event } from '../../../src/base/common/event.js';
import { Disposable } from '../../../src/base/common/lifecycle.js';
import type { IWindowDriver } from '../../../src/workbench/services/driver/windowDriver.js';
import type { BrowserScenarios } from '../../smoke/fixtures/index.js';

/** `F`, answering through a promise, as a call across a process does. */
type RemoteMethod<F> = F extends (...args: infer A) => infer R ? (...args: A) => Promise<Awaited<R>> : never;

/** The methods of `T`, each answering through a promise. */
export type Remote<T> = {
	readonly [K in keyof T as T[K] extends (...args: never[]) => unknown ? K : never]: RemoteMethod<T[K]>;
};

/**
 * Calls `method` of the object at `path` in `window`'s page with `args`,
 * which travel as JSON: the one script the automation sends, and it holds
 * no logic of its own.
 */
function remote<T>(window: BrowserWindow, path: string): Remote<T> {
	const target: object = {};
	return new Proxy(target, {
		get(_target, method) {
			if (typeof method !== 'string') {
				throw new Error('A remote method is named by a string');
			}
			// Not a promise itself: `await` asks for `then`, which no remote object answers.
			if (method === 'then') {
				return undefined;
			}
			return (...args: unknown[]) => window.webContents.executeJavaScript(`${path}[${JSON.stringify(method)}](...${JSON.stringify(args)})`);
		},
	}) as Remote<T>;
}

export class WindowDriver extends Disposable {
	private fixturesLoaded = false;

	/** The workbench's driver in the page (`src/workbench/services/driver`). */
	public readonly renderer: Remote<IWindowDriver>;

	constructor(public readonly window: BrowserWindow, private readonly output: string) {
		super();
		this.renderer = remote<IWindowDriver>(window, 'window.driver');
		this._register(Event.fromNodeEventEmitter<unknown>(window.webContents, 'did-start-navigation')(() => {
			this.fixturesLoaded = false;
		}));
	}

	/**
	 * Waits until the workbench is restored. The page registers its driver as
	 * the workbench starts, so a call before then is asked again, for ten
	 * seconds at most.
	 */
	public async ready(): Promise<void> {
		const deadline = Date.now() + 10000;
		for (; ;) {
			try {
				await this.renderer.whenWorkbenchRestored();
				return;
			} catch (error) {
				if (Date.now() > deadline) {
					throw error;
				}
				await new Promise(resolve => setTimeout(resolve, 100));
			}
		}
	}

	/** The smoke fixtures in the page (`test/smoke/fixtures`), loaded on first use. */
	public async fixtures(): Promise<Remote<BrowserScenarios>> {
		await this.ready();
		if (!this.fixturesLoaded) {
			await this.window.webContents.executeJavaScript(readFileSync(join(__dirname, 'browser-tests.js'), 'utf8'));
			this.fixturesLoaded = true;
		}
		return remote<BrowserScenarios>(this.window, 'browserTests.browserScenarios');
	}

	public async screenshot(name: string): Promise<void> {
		const image = await this.window.webContents.capturePage();
		writeFileSync(join(this.output, `${name}.png`), image.toPNG());
	}
}
