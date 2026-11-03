// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

/**
 * The unit tests: every `*.test.ts` under a `test/<layer>/` folder in `src`,
 * as VS Code places them, run with mocha's TDD interface (`suite`, `test`,
 * `setup`, `teardown`). `test:unit` runs the `common` and `node` layers in
 * Node; `test:unit:electron` runs the `browser`, `electron-browser`,
 * `electron-main` and `electron-utility` layers in a hidden Electron window
 * with Node integration, as VS Code's Electron runner does: the workbench's
 * environment, and Electron's modules for the process layers.
 *
 * VS Code transpiles each file and loads it through its module loader; this
 * build bundles instead, as the application does, so a test imports styles
 * and icons the way its subject does. Node's bundles share one chunk per
 * module (code splitting), so a module has one instance across suites;
 * renderer bundles are one script per test file.
 *
 * Arguments: `--run <file>` (repeatable) limits the run to those test files,
 * and `--grep <pattern>` to tests whose full title matches.
 */
import { build, type BuildOptions } from 'esbuild';
import { mkdir, mkdtemp, readdir, rm, writeFile } from 'node:fs/promises';
import { builtinModules } from 'node:module';
import { join, relative, sep } from 'node:path';
import { pathToFileURL } from 'node:url';
import { electronExecutable } from './electron.ts';
import { isLinux } from './platform.ts';
import { ProcessRunner } from './processes.ts';

/** The layers each environment runs, after VS Code's runners: Node's excludes every Electron and browser layer. */
const nodeLayers = ['common', 'node'];

const rendererLayers = ['browser', 'electron-browser', 'electron-main', 'electron-utility'];

/** Fails a test that runs longer than this, as VS Code's runners do: 5 s locally, 30 s in CI. */
const timeout = process.env.CI ? 30_000 : 5_000;

export interface UnitTestSelection {
	/** Test files relative to the root; every test file of the environment's layers when empty. */
	readonly files: readonly string[];
	readonly grep: string | undefined;
}

/** Reads `--run <file>` and `--grep <pattern>`; anything else is refused. */
export function unitTestSelection(args: readonly string[]): UnitTestSelection {
	const files: string[] = [];
	let grep: string | undefined;
	for (let index = 0; index < args.length; index++) {
		const equals = args[index].indexOf('=');
		const name = equals < 0 ? args[index] : args[index].slice(0, equals);
		const value = (): string => {
			const next = equals < 0 ? args[++index] as string | undefined : args[index].slice(equals + 1);
			if (!next) {
				throw new Error(`${name} needs a value`);
			}
			return next;
		};
		if (name === '--run') {
			files.push(value().split(sep).join('/'));
		} else if (name === '--grep') {
			grep = value();
		} else {
			throw new Error(`Unknown unit test option: ${args[index]}. Options: --run <file>, --grep <pattern>`);
		}
	}
	return { files, grep };
}

/** The test files under `src` whose path has `test/<layer>/` for one of `layers`, sorted. */
export async function unitTestFiles(root: string, layers: readonly string[]): Promise<string[]> {
	const entries = await readdir(join(root, 'src'), { recursive: true, withFileTypes: true });
	return entries
		.filter(entry => entry.isFile() && entry.name.endsWith('.test.ts'))
		.map(entry => relative(root, join(entry.parentPath, entry.name)).split(sep).join('/'))
		.filter(path => {
			const segments = path.split('/');
			return segments.some((segment, index) => segment === 'test' && layers.includes(segments[index + 1]));
		})
		.sort();
}

/** The selected files of `layers`; a `--run` file outside them is refused, so a typo never runs nothing. */
async function selectedFiles(root: string, layers: readonly string[], selection: UnitTestSelection): Promise<string[]> {
	const available = await unitTestFiles(root, layers);
	if (!selection.files.length) {
		return available;
	}
	const unknown = selection.files.filter(file => !available.includes(file));
	if (unknown.length) {
		throw new Error(`Not a ${layers.join(' or ')} unit test: ${unknown.join(', ')}`);
	}
	return [...selection.files];
}

/**
 * A new directory for one run's bundles, inside the checkout's `.build` so
 * that their package imports (`sinon`) resolve from its `node_modules`.
 */
async function outputDirectory(root: string, prefix: string): Promise<string> {
	await mkdir(join(root, '.build'), { recursive: true });
	return mkdtemp(join(root, '.build', prefix));
}

function bundleOptions(root: string, entryPoints: readonly string[], outdir: string): BuildOptions {
	return {
		absWorkingDir: root,
		tsconfig: join(root, 'tsconfig.json'),
		entryPoints: [...entryPoints],
		outdir,
		outbase: root,
		bundle: true,
		sourcemap: 'inline',
		logLevel: 'warning',
		loader: { '.svg': 'text' },
	};
}

/** The `test:unit` task: the `common` and `node` layers under mocha in Node. */
export async function runNodeUnitTests(root: string, signal: AbortSignal, args: readonly string[]): Promise<void> {
	const selection = unitTestSelection(args);
	const files = await selectedFiles(root, nodeLayers, selection);
	if (!files.length) {
		console.log('No node unit tests.');
		return;
	}
	const output = await outputDirectory(root, 'unit-');
	const processes = new ProcessRunner(signal);
	try {
		await build({
			...bundleOptions(root, files, output),
			platform: 'node', format: 'esm', target: 'node22', splitting: true, packages: 'external', outExtension: { '.js': '.mjs' },
			// Workbench styles are the renderer's; Node has no use for them.
			loader: { '.svg': 'text', '.css': 'empty' },
		});
		signal.throwIfAborted();
		const bundles = files.map(file => join(output, file.replace(/\.ts$/, '.mjs')));
		const mocha = join(root, 'node_modules/mocha/bin/mocha.js');
		await processes.run(process.execPath, [mocha, '--ui', 'tdd', '--timeout', String(timeout), ...(selection.grep ? ['--grep', selection.grep] : []), ...bundles], { cwd: root });
	} finally {
		await processes.stop();
		await rm(output, { recursive: true, force: true });
	}
}

/** The `test:unit:electron` task: the `browser` and `electron-browser` layers in a hidden Electron window. */
export async function runElectronUnitTests(root: string, signal: AbortSignal, args: readonly string[]): Promise<void> {
	const selection = unitTestSelection(args);
	const files = await selectedFiles(root, rendererLayers, selection);
	if (!files.length) {
		console.log('No renderer unit tests.');
		return;
	}
	const output = await outputDirectory(root, 'unit-electron-');
	const processes = new ProcessRunner(signal);
	try {
		await build({
			...bundleOptions(root, ['test/unit/electron/main.ts'], output),
			platform: 'node', format: 'cjs', target: 'node22', external: ['electron'], outExtension: { '.js': '.cjs' },
		});
		await build({
			...bundleOptions(root, ['test/unit/electron/renderer.ts', ...files], output),
			// Each test file is one script; Node's modules come from the window's `require`.
			platform: 'browser', format: 'iife', target: 'chrome144', external: [...builtinModules, 'node:*', 'electron'],
			loader: { '.svg': 'text', '.css': 'css' },
		});
		// mocha's browser build defines `suite` and `test` before the runner loads the test bundles.
		const page = `<!DOCTYPE html>
<html>
<head><meta charset="utf-8"><title>VniLink unit tests</title></head>
<body>
<script src="${pathToFileURL(join(root, 'node_modules/mocha/mocha.js')).href}"></script>
<script src="renderer.js"></script>
</body>
</html>
`;
		await writeFile(join(output, 'test/unit/electron/renderer.html'), page);
		signal.throwIfAborted();
		const electron = await electronExecutable(root, signal, { sandboxed: !(isLinux && process.env.VNILINK_TEST_CONTAINER === '1') });
		const containerFlags = isLinux && process.env.VNILINK_TEST_CONTAINER === '1' ? ['--no-sandbox'] : [];
		const profile = join(output, 'profile');
		await processes.run(electron, [join(output, 'test/unit/electron/main.cjs'), ...containerFlags], {
			cwd: root,
			env: {
				...process.env,
				VNILINK_UNIT_OUTPUT: output,
				VNILINK_UNIT_FILES: JSON.stringify(files.map(file => file.replace(/\.ts$/, '.js'))),
				VNILINK_UNIT_GREP: selection.grep ?? '',
				VNILINK_UNIT_TIMEOUT: String(timeout),
				VNILINK_TEST_PROFILE: profile,
			},
		});
	} finally {
		await processes.stop();
		await rm(output, { recursive: true, force: true });
	}
}
