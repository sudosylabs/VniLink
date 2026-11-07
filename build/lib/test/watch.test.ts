// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'assert';
import { watch } from 'node:fs';
import { mkdir, mkdtemp, readFile, rename, rm, writeFile } from 'node:fs/promises';
import { afterEach, beforeEach, suite, test } from 'node:test';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { ApplicationWatcher, replaceFile } from '../watch.ts';

/** Resolves once `file` has content `accept` takes, `undefined` when it does not exist; checked on each change in its folder. */
function whenFile(file: string, accept: (text: string | undefined) => boolean): Promise<void> {
	return new Promise((resolve, reject) => {
		const watcher = watch(dirname(file));
		let settled = false;
		const check = (): void => {
			readFile(file, 'utf8').catch(() => undefined).then(text => {
				if (!settled && accept(text)) {
					settled = true;
					watcher.close();
					resolve();
				}
			}, reject);
		};
		watcher.on('change', check);
		watcher.on('error', reject);
		check();
	});
}

/** A rename that fails with each of `codes` in turn, then succeeds; `calls` counts its attempts. */
function failingRename(codes: readonly string[]): { readonly rename: (from: string, to: string) => Promise<void>; readonly calls: () => number } {
	const remaining = [...codes];
	let calls = 0;
	return {
		calls: () => calls,
		rename: () => {
			calls++;
			const code = remaining.shift();
			return code ? Promise.reject(Object.assign(new Error(code), { code })) : Promise.resolve();
		},
	};
}

/** The watches on the sources that keep this process running: file polls and the esbuild service while a context is open. */
function sourceWatches(): string[] {
	return process.getActiveResourcesInfo().filter(type => type === 'StatWatcher' || type === 'ProcessWrap');
}

suite('watch', () => {
	suite('ApplicationWatcher', { timeout: 30_000 }, () => {
		const entries = ['src/main.ts', 'src/preload.ts', 'src/noticePreload.ts', 'src/notice.ts', 'src/platform/engine/electron-utility/engineMain.ts', 'test/smoke/src/main.ts', 'test/latency/main.ts', 'test/smoke/fixtures/index.ts'];
		let root: string;
		let watcher: ApplicationWatcher;
		/** The first error the watcher reports after startup. */
		let reported: PromiseWithResolvers<unknown>;
		async function put(path: string, text: string): Promise<void> {
			const filename = join(root, path);
			await mkdir(dirname(filename), { recursive: true });
			await writeFile(filename, text);
		}
		const output = (path: string): string => join(root, 'out', path);
		beforeEach(async () => {
			root = await mkdtemp(join(tmpdir(), 'vnilink-watch-'));
			await put('tsconfig.json', '{}');
			for (const path of entries) {
				await put(path, 'console.log("original");');
			}
			await put('src/renderer.ts', 'import "./style.css"; console.log("original");');
			await put('src/style.css', 'body { color: red; }');
			await put('src/workbench/index.html', '<title>original</title>');
			await put('src/workbench/notice.html', '<title>notice</title>');
			await put('assets/desktop/app-icon.svg', '<svg/>');
			await put('node_modules/@phosphor-icons/core/LICENSE', 'phosphor');
			await put('LICENSE.vscode.txt', 'MIT');
			await put('ThirdPartyNotices.vscode.txt', 'notices');
			await put('out/native/vnilink.node', 'native-sentinel');
			reported = Promise.withResolvers();
			watcher = await new ApplicationWatcher(root, error => {
				reported.resolve(error);
			}).start();
		});
		afterEach(async () => {
			await watcher.dispose();
			await rm(root, { recursive: true, force: true });
		});
		test('rebuilds the renderer when its TypeScript changes', async () => {
			await put('src/renderer.ts', 'import "./style.css"; console.log("updated-renderer");');
			await whenFile(output('renderer/renderer.js'), text => text?.includes('updated-renderer') === true);
		});
		test('rebuilds the renderer stylesheet when its CSS changes', async () => {
			await put('src/style.css', 'body { color: blue; }');
			await whenFile(output('renderer/renderer.css'), text => text?.includes('color: blue') === true);
		});
		test('rebuilds the main process bundle when its TypeScript changes', async () => {
			await put('src/main.ts', 'console.log("updated-main");');
			await whenFile(output('main.cjs'), text => text?.includes('updated-main') === true);
		});
		test('copies a page saved by renaming a new file over it', async () => {
			await put('src/workbench/browser/replacement.html', '<title>atomic-save</title>');
			await rename(join(root, 'src/workbench/browser/replacement.html'), join(root, 'src/workbench/index.html'));
			await whenFile(output('renderer/index.html'), text => text?.includes('atomic-save') === true);
		});
		test('removes the copy of a deleted page', async () => {
			await rm(join(root, 'src/workbench/index.html'));
			await whenFile(output('renderer/index.html'), text => text === undefined);
		});
		test('copies a deleted page again once it is restored', async () => {
			await rm(join(root, 'src/workbench/index.html'));
			await whenFile(output('renderer/index.html'), text => text === undefined);
			await put('src/workbench/index.html', '<title>restored</title>');
			await whenFile(output('renderer/index.html'), text => text?.includes('restored') === true);
		});
		test('reports a page it cannot copy', async () => {
			await rm(output('renderer'), { recursive: true, force: true });
			await put('out/renderer', 'a file where the renderer folder belongs');
			await put('src/workbench/index.html', '<title>uncopyable</title>');
			const error = await reported.promise;
			assert.match(String((error as NodeJS.ErrnoException).code), /^(EEXIST|ENOTDIR)$/);
		});
		test('leaves the native addon alone when Rust changes', async () => {
			await put('crates/example/src/lib.rs', 'this deliberately is not valid Rust');
			// A later TypeScript rebuild shows the watcher has seen the Rust change.
			await put('src/main.ts', 'console.log("after-rust");');
			await whenFile(output('main.cjs'), text => text?.includes('after-rust') === true);
			assert.strictEqual(await readFile(output('native/vnilink.node'), 'utf8'), 'native-sentinel');
		});
		test('disposing ends every watch on the sources', async () => {
			assert.notDeepStrictEqual(sourceWatches(), []);
			await watcher.dispose();
			assert.deepStrictEqual(sourceWatches(), []);
		});
		test('a disposed watcher refuses to start again', async () => {
			await watcher.dispose();
			assert.throws(() => watcher.start(), /closed/);
		});
	});
	suite('replaceFile', () => {
		for (const code of ['EPERM', 'EBUSY', 'EACCES']) {
			test(`tries the rename again after ${code}`, async () => {
				const busy = failingRename([code]);
				await replaceFile('a.partial', 'a', busy.rename);
				assert.strictEqual(busy.calls(), 2);
			});
		}
		test('tries the rename until the file is free', async () => {
			const busy = failingRename(['EPERM', 'EBUSY', 'EACCES']);
			await replaceFile('a.partial', 'a', busy.rename);
			assert.strictEqual(busy.calls(), 4);
		});
		test('rejects with any other rename error', async () => {
			await assert.rejects(replaceFile('a.partial', 'a', failingRename(['ENOENT']).rename), /ENOENT/);
		});
		test('does not try again after any other rename error', async () => {
			const missing = failingRename(['ENOENT']);
			await replaceFile('a.partial', 'a', missing.rename).catch(() => undefined);
			assert.strictEqual(missing.calls(), 1);
		});
		test('rejects once the file stays in use', async () => {
			await assert.rejects(replaceFile('a.partial', 'a', failingRename(Array<string>(25).fill('EPERM')).rename), /EPERM/);
		});
		test('gives up after twenty attempts, about two seconds', async () => {
			const held = failingRename(Array<string>(25).fill('EPERM'));
			await replaceFile('a.partial', 'a', held.rename).catch(() => undefined);
			assert.strictEqual(held.calls(), 20);
		});
	});
});
