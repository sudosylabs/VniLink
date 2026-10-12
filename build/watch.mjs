import { context } from 'esbuild';
import { watchFile, unwatchFile } from 'node:fs';
import { cp, mkdir, rm } from 'node:fs/promises';
import { basename, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import { buildOptions, resources } from './tasks.mjs';

// Like VS Code's desktop watcher: emit changes; the developer reloads the window.
// Build and watch share output definitions, including CSS imported by TypeScript.
export async function watchApplication(root = fileURLToPath(new URL('../', import.meta.url))) {
	const contexts = [];
	const watchers = [];
	const copies = new Map();
	let stopping = false;
	async function copy(resource) {
		const previous = copies.get(resource.source) ?? Promise.resolve();
		const next = previous.then(async () => {
			await mkdir(dirname(resource.output), { recursive: true });
			try {
				await cp(resource.source, resource.output);
				console.log(`[watch] Copied ${basename(resource.source)}`);
			} catch (error) {
				if (error.code === 'ENOENT') {
					await rm(resource.output, { force: true });
				} else {
					throw error;
				}
			}
		}).catch(error => console.error('[watch] Resource copy failed:', error));
		copies.set(resource.source, next);
		await next;
	}
	async function dispose() {
		stopping = true;
		for (const watcher of watchers) {
			watcher.close();
		}
		await Promise.all(contexts.map(ctx => ctx.dispose()));
		await Promise.all(copies.values());
	}
	try {
		for (const resource of resources(root)) {
			// Poll the small resource set, like esbuild's source watcher. This also
			// handles atomic editor saves without consuming native directory watches.
			const changed = () => {
				if (!stopping) {
					void copy(resource);
				}
			};
			watchFile(resource.source, { interval: 200 }, changed);
			watchers.push({ close: () => unwatchFile(resource.source, changed) });
			await copy(resource);
		}
		for (const options of buildOptions(root)) {
			const label = basename(options.outfile);
			const ctx = await context({
				...options, sourcemap: 'inline', plugins: [{
					name: 'watch-status', setup(build) {
						build.onStart(() => console.log(`[watch] Building ${label}`));
						build.onEnd(result => console.log(`[watch] Finished ${label} with ${result.errors.length} errors`));
					}
				}]
			});
			contexts.push(ctx);
			await ctx.rebuild();
			await ctx.watch();
		}
		console.log('[watch] Ready. Reload Window: Cmd+R / Ctrl+R. Restart Electron for main-process changes.');
		return { dispose };
	} catch (error) {
		await dispose();
		throw error;
	}
}
