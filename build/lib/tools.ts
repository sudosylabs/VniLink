// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

/**
 * The checks that are another program's run: the TypeScript compiler, the
 * linters, Node's test runner and Cargo's formatter and linter. Each runs in
 * the checkout under the task's signal, with its output on this terminal.
 */
import { createRequire } from 'node:module';
import { dirname, join } from 'node:path';
import { readFile } from 'node:fs/promises';
import { ProcessRunner } from './processes.ts';

/** Runs `command` in `root` and resolves when it succeeds. */
export async function runTool(root: string, signal: AbortSignal, command: string, args: readonly string[]): Promise<void> {
	const processes = new ProcessRunner(signal);
	try {
		await processes.run(command, args, { cwd: root });
	} finally {
		await processes.stop();
	}
}

/** The script a dependency installs as `bin`, run with this Node. */
async function binary(root: string, packageName: string, name = packageName): Promise<string> {
	const manifestPath = createRequire(join(root, 'package.json')).resolve(`${packageName}/package.json`);
	const manifest = JSON.parse(await readFile(manifestPath, 'utf8')) as { bin?: string | Record<string, string> };
	const bin = typeof manifest.bin === 'string' ? manifest.bin : manifest.bin?.[name];
	if (!bin) {
		throw new Error(`${packageName} installs no ${name} command`);
	}
	return join(dirname(manifestPath), bin);
}

async function runNodeTool(root: string, signal: AbortSignal, packageName: string, args: readonly string[], name?: string): Promise<void> {
	await runTool(root, signal, process.execPath, [await binary(root, packageName, name), ...args]);
}

/** The `typecheck` task: the application and tests, then the build. */
export async function typecheck(root: string, signal: AbortSignal): Promise<void> {
	await runNodeTool(root, signal, 'typescript', ['--noEmit'], 'tsc');
	await runNodeTool(root, signal, 'typescript', ['-p', 'build/tsconfig.json'], 'tsc');
}

export async function lintTypeScript(root: string, signal: AbortSignal, fix = false): Promise<void> {
	await runNodeTool(root, signal, 'eslint', ['.', ...(fix ? ['--fix'] : ['--max-warnings', '0'])]);
}

export async function lintStyles(root: string, signal: AbortSignal, fix = false): Promise<void> {
	await runNodeTool(root, signal, 'stylelint', ['src/**/*.css', ...(fix ? ['--fix'] : ['--max-warnings', '0'])]);
}

/** Node's test runner over `pattern`, which it expands itself. */
export async function runNodeTests(root: string, signal: AbortSignal, pattern: string): Promise<void> {
	await runTool(root, signal, process.execPath, ['--test', pattern]);
}

/** The `check:rust` task: the Rust workspace is formatted and lint-free. */
export async function checkRust(root: string, signal: AbortSignal): Promise<void> {
	await runTool(root, signal, 'cargo', ['fmt', '--all', '--', '--check']);
	await runTool(root, signal, 'cargo', ['clippy', '--workspace', '--all-targets', '--locked', '--', '-D', 'warnings']);
}
