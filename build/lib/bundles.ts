// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

/** The application's bundles and resources, shared by the one-shot, release and watch builds. */
import { build, type BuildOptions } from 'esbuild';
import { cp, mkdir, readFile, rename, rm } from 'node:fs/promises';
import { basename, dirname, join, relative } from 'node:path';
import { acquireLock } from './lock.ts';
import { compileNative, defaultNativeOptions, stageGlass, stageSpdlog, stageNative, type NativeIdentity, type NativeOptions } from './native.ts';
import { ProcessRunner } from './processes.ts';
import { define, execute, parallel, series } from './task.ts';

export interface BundleOptions {
	readonly production?: boolean;
	readonly outputDirectory?: string;
}

export interface Resource {
	readonly source: string;
	readonly output: string;
}

export function bundleOptions(root: string, { production = false, outputDirectory = join(root, 'out') }: BundleOptions = {}): BuildOptions[] {
	const common: BuildOptions = { absWorkingDir: root, tsconfig: join(root, 'tsconfig.json'), bundle: true, ...(production ? { minify: true, keepNames: true, define: { 'process.env.NODE_ENV': '"production"' } } : {}) };
	const node = (entry: string, output: string): BuildOptions => ({ ...common, entryPoints: [entry], outfile: join(outputDirectory, output), platform: 'node', format: 'cjs', target: 'node22', external: ['electron'] });
	const browser = (entry: string, output: string, extra: BuildOptions = {}): BuildOptions => ({ ...common, entryPoints: [entry], outfile: join(outputDirectory, output), platform: 'browser', target: 'chrome144', loader: { '.svg': 'text' }, ...extra });
	return [
		node('src/main.ts', 'main.cjs'),
		node('src/platform/engine/electron-utility/engineMain.ts', 'engine.cjs'),
		node('src/preload.ts', 'preload.cjs'),
		// The notice window: its own page, bundle and preload, apart from the workbench's IPC.
		node('src/noticePreload.ts', 'noticePreload.cjs'),
		...(!production ? [node('test/smoke/src/main.ts', 'smoke.cjs'), node('test/latency/main.ts', 'latency.cjs')] : []),
		browser('src/renderer.ts', 'renderer/renderer.js'),
		browser('src/notice.ts', 'renderer/notice.js'),
		...(!production ? [browser('test/smoke/fixtures/index.ts', 'browser-tests.js', { format: 'iife', globalName: 'browserTests' })] : []),
	];
}

/**
 * Throws unless `out/<output>` and each file written beside it, such as the
 * stylesheet its imports gather, are exactly what building its sources now
 * writes: the test agent's revision covers a side's sources, so the script
 * it launches must be built from them, not left from another commit.
 */
export async function assertBundleFresh(root: string, output: string, outputDirectory = join(root, 'out')): Promise<void> {
	const outfile = join(outputDirectory, output);
	const options = bundleOptions(root, { outputDirectory }).find(candidate => candidate.outfile === outfile);
	if (!options) {
		throw new Error(`No bundle writes out/${output}`);
	}
	const result = await build({ ...options, write: false, logLevel: 'silent' });
	if (!result.outputFiles.some(file => file.path === outfile)) {
		throw new Error(`Building out/${output} wrote no such file`);
	}
	for (const built of result.outputFiles) {
		const current = await readFile(built.path).catch(() => undefined);
		if (!current || !Buffer.from(built.contents).equals(current)) {
			throw new Error(`out/${relative(outputDirectory, built.path)} is missing or was not built from these sources: run npm run build`);
		}
	}
}

/** Throws unless every bundle in `out/` is what building its sources now writes. */
export async function assertBundlesFresh(root: string): Promise<void> {
	await Promise.all(bundleOptions(root).map(options => assertBundleFresh(root, relative(join(root, 'out'), options.outfile ?? ''))));
}

/**
 * What a build copies beside its bundles. A release also carries the Rust
 * sources and lockfile its addon can be relinked from; a development build
 * and the watcher leave them out, since nothing in a checkout reads them.
 */
export function resources(root: string, outputDirectory = join(root, 'out'), { relink = false } = {}): Resource[] {
	const relinkSources = relink ? [
		['Cargo.toml', 'relink/source/Cargo.toml'],
		['Cargo.lock', 'relink/source/Cargo.lock'],
		['.cargo/config.toml', 'relink/source/.cargo/config.toml'],
		['rust-toolchain.toml', 'relink/source/rust-toolchain.toml'],
		['crates', 'relink/source/crates'],
		['vendor', 'relink/source/vendor'],
		// The packages each OS builds with, which the notice points to.
		['build/prerequisites.json', 'relink/source/build/prerequisites.json'],
	] : [];
	return [
		['src/workbench/index.html', 'renderer/index.html'],
		['src/workbench/notice.html', 'renderer/notice.html'],
		['assets/desktop/app-icon.svg', 'renderer/app-icon.svg'],
		// Tray icons, loaded by the main process next to its bundle.
		['assets/tray', 'assets/tray'],
		['node_modules/@phosphor-icons/core/LICENSE', 'licenses/phosphor-icons.txt'],
		// xterm.js and its addons are bundled into the renderer; each package carries its own notice.
		['node_modules/@xterm/xterm/LICENSE', 'licenses/xterm.txt'],
		['node_modules/@xterm/addon-webgl/LICENSE', 'licenses/xterm-addon-webgl.txt'],
		['node_modules/@xterm/addon-unicode11/LICENSE', 'licenses/xterm-addon-unicode11.txt'],
		['LICENSE', 'licenses/vnilink.txt'],
		['LICENSE.vscode.txt', 'licenses/vscode.txt'],
		['ThirdPartyNotices.vscode.txt', 'licenses/vscode-third-party.txt'],
		['assets/licenses', 'licenses/native-codecs'],
		...relinkSources,
	].map(([source, output]) => ({ source: join(root, source), output: join(outputDirectory, output) }));
}

const generatedDirectories = new Set(['.git', '.build', 'target', 'node_modules', '__pycache__']);

/**
 * A cargo-fuzz crate is outside the workspace, so no relink input; its
 * build and corpus are large, and a running fuzzer rewrites the corpus.
 */
export function relinkSourceFilter(source: string): boolean {
	return basename(source) !== 'fuzz';
}

/** Only source inputs belong in relinking material; generated files are never build inputs. */
export function releaseSourceFilter(source: string): boolean {
	const name = basename(source);
	return relinkSourceFilter(source) && !generatedDirectories.has(name) && name !== 'AGENTS.md' && !/\.(o|obj|a|so|dylib|dll|node|exe|pyc)$/.test(name);
}

export interface ApplicationBuildOptions extends BundleOptions {
	readonly native?: NativeOptions;
}

/**
 * Compiles the addon, then bundles, stages resources and publishes the addon
 * into `outputDirectory`. The output is built beside it (`out.next` for
 * `out`) and renamed into place, so a build that fails or is stopped leaves
 * the previous output whole; a lock keeps two builds from publishing at once.
 */
export async function buildApplication(root: string, signal: AbortSignal, options: ApplicationBuildOptions = {}): Promise<void> {
	const { production = false } = options;
	const published = options.outputDirectory ?? join(root, 'out');
	const outputDirectory = join(dirname(published), `${basename(published)}.next`);
	const native = options.native ?? defaultNativeOptions();
	const processes = new ProcessRunner(signal);
	let compiled: NativeIdentity | undefined;
	const task = series(
		define('native:compile', async () => {
			compiled = await compileNative(root, processes, native);
		}),
		define('output:prepare', async () => {
			await rm(outputDirectory, { recursive: true, force: true });
			await Promise.all(['renderer', 'native', 'licenses'].map(name => mkdir(join(outputDirectory, name), { recursive: true })));
		}),
		parallel(
			define('typescript:bundle', async () => {
				await Promise.all(bundleOptions(root, { production, outputDirectory }).map(value => build(value)));
			}),
			define('resources:stage', async () => {
				const staged = resources(root, outputDirectory, { relink: production });
				await Promise.all(staged.map(({ source, output }) => cp(source, output, { recursive: true, filter: releaseSourceFilter })));
			}),
			define('native:stage', async () => {
				if (!compiled) {
					throw new Error('The native addon was not compiled');
				}
				await stageNative(root, outputDirectory, native, compiled);
			}),
			define('glass:stage', async () => {
				await stageGlass(root, outputDirectory);
			}),
			define('spdlog:stage', async () => {
				await stageSpdlog(root, outputDirectory);
			}),
		),
	);
	await mkdir(dirname(published), { recursive: true });
	const releaseLock = await acquireLock(`${published}.lock`);
	try {
		await execute(define('application:build', task), signal);
		signal.throwIfAborted();
		await rm(published, { recursive: true, force: true });
		await rename(outputDirectory, published);
	} finally {
		await processes.stop();
		await rm(outputDirectory, { recursive: true, force: true });
		await releaseLock();
	}
}
