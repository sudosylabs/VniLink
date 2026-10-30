// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

/** The application's bundles and resources, shared by the one-shot, release and watch builds. */
import { build, type BuildOptions } from 'esbuild';
import { cp, mkdir, readFile, rm } from 'node:fs/promises';
import { basename, join } from 'node:path';
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
		// The restart notice: its own page, bundle and preload, apart from the workbench's IPC.
		node('src/noticePreload.ts', 'noticePreload.cjs'),
		...(!production ? [node('test/smoke/main.ts', 'smoke.cjs'), node('test/latency/main.ts', 'latency.cjs')] : []),
		browser('src/renderer.ts', 'renderer/renderer.js'),
		browser('src/notice.ts', 'renderer/notice.js'),
		...(!production ? [browser('test/browser/index.ts', 'browser-tests.js', { format: 'iife', globalName: 'browserTests' })] : []),
	];
}

/**
 * Throws unless `out/<output>` is exactly what building its sources now
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
	const built = result.outputFiles.find(file => file.path === outfile);
	const current = await readFile(outfile).catch(() => undefined);
	if (!built || !current || !Buffer.from(built.contents).equals(current)) {
		throw new Error(`out/${output} is missing or was not built from these sources: run npm run build`);
	}
}

export function resources(root: string, outputDirectory = join(root, 'out')): Resource[] {
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
		['Cargo.toml', 'relink/source/Cargo.toml'],
		['Cargo.lock', 'relink/source/Cargo.lock'],
		['.cargo/config.toml', 'relink/source/.cargo/config.toml'],
		['rust-toolchain.toml', 'relink/source/rust-toolchain.toml'],
		['crates', 'relink/source/crates'],
		['vendor', 'relink/source/vendor'],
		['.github/workflows/ci.yml', 'relink/source/.github/workflows/ci.yml'],
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

/** Compiles the addon, then bundles, stages resources and publishes the addon into `outputDirectory`. */
export async function buildApplication(root: string, signal: AbortSignal, options: ApplicationBuildOptions = {}): Promise<void> {
	const { production = false, outputDirectory = join(root, 'out') } = options;
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
				const filter = production ? releaseSourceFilter : relinkSourceFilter;
				await Promise.all(resources(root, outputDirectory).map(({ source, output }) => cp(source, output, { recursive: true, filter })));
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
	try {
		await execute(define('application:build', task), signal);
	} finally {
		await processes.stop();
	}
}
