/** Emits renderer/main changes; the developer reloads or restarts the appropriate process. */
import { context } from 'esbuild';
import { watchFile, unwatchFile } from 'node:fs';
import { cp, mkdir, rm } from 'node:fs/promises';
import { basename, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import { buildOptions, resources } from './tasks.mjs';

export class ApplicationWatcher {
	#contexts = [];
	#watchers = [];
	#copies = new Map();
	#stopping = false;
	#started;
	#disposed;

	constructor(root = fileURLToPath(new URL('../', import.meta.url)), onError = error => console.error('[watch]', error)) {
		this.root = root;
		this.onError = onError;
	}

	start() {
		if (this.#stopping) {
			throw new Error('Application watcher is closed');
		}
		this.#started ??= this.#start();
		return this.#started;
	}

	async #start() {
		try {
			for (const resource of resources(this.root)) {
				this.#checkAlive();
				const changed = () => {
					if (!this.#stopping) {
						void this.#copy(resource).catch(this.onError);
					}
				};
				watchFile(resource.source, { interval: 200 }, changed);
				this.#watchers.push(() => unwatchFile(resource.source, changed));
				await this.#copy(resource);
			}
			for (const options of buildOptions(this.root)) {
				this.#checkAlive();
				const label = basename(options.outfile);
				const ctx = await context({
					...options, sourcemap: 'inline', plugins: [{
						name: 'watch-status', setup: build => {
							build.onStart(() => console.log(`[watch] Building ${label}`));
							build.onEnd(result => console.log(`[watch] Finished ${label} with ${result.errors.length} errors`));
						}
					}]
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

	#copy(resource) {
		const previous = this.#copies.get(resource.source) ?? Promise.resolve();
		const next = previous.catch(() => undefined).then(async () => {
			await mkdir(dirname(resource.output), { recursive: true });
			try {
				await cp(resource.source, resource.output, { recursive: true });
			} catch (error) {
				if (error.code !== 'ENOENT') {
					throw error;
				}
				await rm(resource.output, { recursive: true, force: true });
			}
		});
		this.#copies.set(resource.source, next);
		return next;
	}

	#checkAlive() {
		if (this.#stopping) {
			throw new Error('Application watcher stopped during startup');
		}
	}

	async #cleanup() {
		for (const close of this.#watchers.splice(0)) {
			close();
		}
		await Promise.all(this.#contexts.splice(0).map(ctx => ctx.dispose()));
		await Promise.allSettled(this.#copies.values());
	}

	dispose() {
		this.#stopping = true;
		this.#disposed ??= this.#cleanup().then(async () => {
			await this.#started?.catch(() => undefined);
		});
		return this.#disposed;
	}
}

export async function watchApplication(root) {
	return await new ApplicationWatcher(root).start();
}
