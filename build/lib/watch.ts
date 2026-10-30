// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

/** Emits renderer/main changes; the developer reloads or restarts the appropriate process. */
import { context, type BuildContext } from 'esbuild';
import { unwatchFile, watchFile } from 'node:fs';
import { copyFile, cp, mkdir, rename, rm, stat } from 'node:fs/promises';
import { basename, dirname } from 'node:path';
import { bundleOptions, resources, type Resource } from './bundles.ts';

/** Errors Windows gives renaming over a file another process has open, for a moment. */
const IN_USE = new Set(['EPERM', 'EACCES', 'EBUSY']);

/**
 * Renames `from` over `to`, in one step. Windows refuses while another
 * process has `to` open, as a window reloading the page does for a moment;
 * the rename is tried again, waiting a little longer each time, for about
 * two seconds. Any other error is final.
 */
export async function replaceFile(from: string, to: string, renameFile: (from: string, to: string) => Promise<void> = rename): Promise<void> {
	for (let attempt = 1; ; attempt++) {
		try {
			await renameFile(from, to);
			return;
		} catch (error) {
			if (attempt >= 20 || !IN_USE.has(String((error as NodeJS.ErrnoException).code))) {
				throw error;
			}
			await new Promise(resolve => setTimeout(resolve, attempt * 10));
		}
	}
}

export class ApplicationWatcher {
	readonly #root: string;
	readonly #onError: (error: unknown) => void;
	readonly #contexts: BuildContext[] = [];
	readonly #watchers: (() => void)[] = [];
	readonly #copies = new Map<string, Promise<void>>();
	#stopping = false;
	#started: Promise<this> | undefined;
	#disposed: Promise<void> | undefined;

	constructor(root: string, onError: (error: unknown) => void = error => {
		console.error('[watch]', error);
	}) {
		this.#root = root;
		this.#onError = onError;
	}

	public start(): Promise<this> {
		if (this.#stopping) {
			throw new Error('Application watcher is closed');
		}
		this.#started ??= this.#start();
		return this.#started;
	}

	async #start(): Promise<this> {
		try {
			for (const resource of resources(this.#root)) {
				this.#checkAlive();
				const changed = (): void => {
					if (!this.#stopping) {
						this.#copy(resource).catch(this.#onError);
					}
				};
				watchFile(resource.source, { interval: 200 }, changed);
				this.#watchers.push(() => {
					unwatchFile(resource.source, changed);
				});
				await this.#copy(resource);
			}
			for (const options of bundleOptions(this.#root)) {
				this.#checkAlive();
				const label = basename(options.outfile ?? '');
				const ctx = await context({
					...options, sourcemap: 'inline', plugins: [{
						name: 'watch-status', setup: build => {
							build.onStart(() => {
								console.log(`[watch] Building ${label}`);
							});
							build.onEnd(result => {
								console.log(`[watch] Finished ${label} with ${String(result.errors.length)} errors`);
							});
						},
					}],
				});
				if (this.#stopping) {
					await ctx.dispose();
					this.#checkAlive();
				}
				this.#contexts.push(ctx);
				await ctx.rebuild();
				this.#checkAlive();
				await ctx.watch();
			}
			console.log('[watch] Ready. Reload Window: Cmd+R / Ctrl+R. Restart Electron for main-process changes.');
			return this;
		} catch (error) {
			this.#stopping = true;
			await this.#cleanup();
			throw error;
		}
	}

	#copy(resource: Resource): Promise<void> {
		const previous = this.#copies.get(resource.source) ?? Promise.resolve();
		const next = previous.catch(() => undefined).then(async () => {
			await mkdir(dirname(resource.output), { recursive: true });
			try {
				if ((await stat(resource.source)).isDirectory()) {
					await cp(resource.source, resource.output, { recursive: true });
				} else {
					// `cp` unlinks the output before copying, so a reload in between
					// finds no page; the renamed copy replaces it in one step.
					const partial = `${resource.output}.partial`;
					await copyFile(resource.source, partial);
					await replaceFile(partial, resource.output);
				}
			} catch (error) {
				if ((error as NodeJS.ErrnoException).code !== 'ENOENT') {
					throw error;
				}
				await rm(resource.output, { recursive: true, force: true });
			}
		});
		this.#copies.set(resource.source, next);
		return next;
	}

	#checkAlive(): void {
		if (this.#stopping) {
			throw new Error('Application watcher stopped during startup');
		}
	}

	async #cleanup(): Promise<void> {
		for (const close of this.#watchers.splice(0)) {
			close();
		}
		await Promise.all(this.#contexts.splice(0).map(ctx => ctx.dispose()));
		await Promise.allSettled(this.#copies.values());
	}

	public dispose(): Promise<void> {
		this.#stopping = true;
		this.#disposed ??= this.#cleanup().then(async () => {
			await this.#started?.catch(() => undefined);
		});
		return this.#disposed;
	}
}

export function watchApplication(root: string): Promise<ApplicationWatcher> {
	return new ApplicationWatcher(root).start();
}
