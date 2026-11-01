// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

/**
 * Launches an unmodified assembled application outside the source tree. It
 * checks itself (`--self-check`, `src/code/electron-main/selfCheck.ts`) with
 * a temporary engine and profile; the build also confirms it refuses the
 * smoke-test driver and remote debugging that a shipped build must not offer.
 */
import assert from 'node:assert/strict';
import { cp, mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { release, tmpdir } from 'node:os';
import { join } from 'node:path';
import { assemblyEnvironment, packagedLayout } from './package.ts';
import { ProcessRunner } from './processes.ts';
import { readProduct } from './product.ts';
import { verifyPackageFiles } from './verify.ts';

/** What `--self-check` prints; see `SelfCheckResult` in the application. */
interface SelfCheckResult {
	readonly passed: boolean;
	readonly problems: readonly string[];
	readonly version: string;
	readonly platform: string;
	readonly architecture: string;
	readonly material: string | undefined;
}

/** How long one launch of the packaged application may take. */
const launchTimeout = 120_000;

/** Runs the packaged executable with `args` and returns its exit code and output. */
async function launch(executable: string, args: readonly string[], cwd: string, signal: AbortSignal): Promise<{ code: number | undefined; output: string }> {
	const processes = new ProcessRunner(AbortSignal.any([signal, AbortSignal.timeout(launchTimeout)]));
	let output = '';
	try {
		const launched = processes.start(executable, args, { cwd, env: assemblyEnvironment(process.env), stdio: ['ignore', 'pipe', 'pipe'] });
		const collect = (bytes: Buffer): void => {
			output = (output + bytes.toString()).slice(-65536);
		};
		launched.child.stdout?.on('data', collect);
		launched.child.stderr?.on('data', collect);
		const code = await launched.result.then(() => 0, () => launched.child.exitCode ?? undefined);
		return { code, output };
	} finally {
		await processes.stop();
	}
}

/** Copies the application in `directory` to a temporary location and checks that it runs, and refuses what it must. */
export async function runPackagedApplication(root: string, directory: string, signal: AbortSignal): Promise<void> {
	const product = await readProduct(root);
	const manifest = await verifyPackageFiles(directory, product);
	const work = await mkdtemp(join(tmpdir(), 'vnilink-package-check-'));
	const artifacts = join(root, '.build/package-tests', `${String(Date.now())}-${String(process.pid)}`);
	const log: string[] = [];
	await mkdir(artifacts, { recursive: true });
	try {
		const copy = join(work, 'application');
		await cp(directory, copy, { recursive: true, verbatimSymlinks: true });
		const { executable } = packagedLayout(copy, process.platform, product);
		for (const refused of [['--smoke-test'], ['--remote-debugging-port=0']]) {
			const { code, output } = await launch(executable, refused, work, signal);
			log.push(`$ ${refused.join(' ')} -> ${String(code)}\n${output}`);
			assert.equal(code, 2, `The packaged application did not refuse ${refused.join(' ')}`);
		}
		const { code, output } = await launch(executable, [`--self-check=${artifacts}`], work, signal);
		log.push(`$ --self-check -> ${String(code)}\n${output}`);
		const result = JSON.parse(await readFile(join(artifacts, 'result.json'), 'utf8')) as SelfCheckResult;
		assert.equal(code, result.passed ? 0 : 1);
		assert.deepEqual(result.problems, [], 'The packaged application failed its self-check');
		assert.equal(result.version, manifest.version);
		assert.equal(result.platform, process.platform);
		// macOS 26 (Darwin 25) and later ship Liquid Glass; vibrancy there means elg did not load
		// from the package. Reduce Transparency may still turn the material off.
		if (process.platform === 'darwin' && Number(release().split('.')[0]) >= 25) {
			assert.ok(result.material === 'glass' || result.material === 'none', `The packaged window has ${String(result.material)} instead of Liquid Glass`);
		}
		console.log(`Packaged application passed isolated launch checks; evidence: ${artifacts}`);
	} finally {
		await writeFile(join(artifacts, 'application.log'), log.join('\n'));
		await rm(work, { recursive: true, force: true });
	}
}
