// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

/**
 * The project's Linux container, which Linux validation and the
 * impaired-network measurements run in: one image, the checkout mounted
 * read-only, an evidence folder, and writable caches kept per checkout.
 */
import { createHash } from 'node:crypto';
import { mkdir, readFile } from 'node:fs/promises';
import { join } from 'node:path';
import { ProcessRunner } from './processes.ts';

const DOCKERFILE = 'build/containers/Dockerfile.linux';

/** The image every project container runs, built from `DOCKERFILE`. */
const IMAGE = 'vnilink-linux-validation:local';

/**
 * A cache volume for one checkout: worktrees of the same repository build
 * different code, and a shared target would let one run link or execute
 * another's artifacts.
 */
export function checkoutVolume(name: string, root: string): string {
	return `${name}-${createHash('sha256').update(root).digest('hex').slice(0, 12)}`;
}

/**
 * The distribution and Rust minor release the image builds with, as its
 * Dockerfile's `FROM rust:<release>-<distribution>` names them: `trixie-rust191`
 * for `rust:1.91.0-trixie`. A patch release links the same artifacts.
 */
export function imageToolchain(dockerfile: string): string {
	const match = /^FROM rust:(\d+)\.(\d+)[\d.]*-([a-z]+)[@\s]/m.exec(dockerfile);
	if (!match) {
		throw new Error(`${DOCKERFILE} has no FROM rust:<release>-<distribution> line`);
	}
	return `${match[3]}-rust${match[1]}${match[2]}`;
}

/**
 * The writable caches of a container run for `purpose`. The Cargo registry is
 * shared: its downloads are content-addressed and Cargo locks them. The target
 * and `node_modules` hold this checkout's build, so each checkout has its own,
 * and the target is named for the toolchain, whose artifacts another cannot link.
 */
export function containerVolumes(purpose: string, toolchain: string, root: string): string[] {
	return [
		'--mount', 'type=volume,source=vnilink-linux-cargo-registry,target=/usr/local/cargo/registry',
		'--mount', `type=volume,source=${checkoutVolume(`vnilink-${purpose}-${toolchain}-target`, root)},target=/work/target`,
		'--mount', `type=volume,source=${checkoutVolume(`vnilink-${purpose}-node-modules`, root)},target=/work/node_modules`,
	];
}

export interface ContainerRun {
	/** Names the container (`vnilink-<purpose>-…`), its caches and, under `.build`, its evidence folder. */
	readonly purpose: string;
	/** The evidence folder's name under `.build`. */
	readonly evidence: string;
	/** `docker run` options after the shared mounts. */
	readonly options: readonly string[];
	/** What follows the image: the entrypoint's arguments. */
	readonly args?: readonly string[];
	/** Checks made once Docker answers, before the image is built. */
	readonly beforeBuild?: (processes: ProcessRunner) => Promise<void>;
}

/**
 * Builds the image and runs it once with the checkout read-only at `/source`
 * and a new evidence folder at `/evidence`; the container is removed however
 * the run ends.
 */
export async function runContainer(root: string, signal: AbortSignal, run: ContainerRun): Promise<void> {
	const output = join(root, '.build', run.evidence, String(Date.now()));
	const container = `vnilink-${run.purpose}-${String(process.pid)}-${String(Date.now())}`;
	const toolchain = imageToolchain(await readFile(join(root, DOCKERFILE), 'utf8'));
	const processes = new ProcessRunner(signal);
	let launched = false;
	await mkdir(output, { recursive: true });
	try {
		await processes.run('docker', ['info', '--format', '{{.OSType}} {{.Architecture}}']);
		await run.beforeBuild?.(processes);
		await processes.run('docker', ['build', '-f', DOCKERFILE, '-t', IMAGE, '.'], { cwd: root });
		launched = true;
		await processes.run('docker', [
			'run', '--name', container, '--init',
			'--mount', `type=bind,source=${root},target=/source,readonly`,
			'--mount', `type=bind,source=${output},target=/evidence`,
			...containerVolumes(run.purpose, toolchain, root),
			...run.options,
			IMAGE, ...run.args ?? [],
		]);
	} finally {
		await processes.stop();
		if (launched) {
			// Removal uses a separate owner because cancellation closes the work runner.
			const cleanup = new ProcessRunner();
			try {
				await cleanup.run('docker', ['rm', '--force', container]);
			} finally {
				await cleanup.stop();
			}
		}
		console.log(`Evidence: ${output}`);
	}
}
