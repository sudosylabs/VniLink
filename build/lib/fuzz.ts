// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

/**
 * The `fuzz` task: runs the cargo-fuzz targets of `crates/vnilink/fuzz`
 * (`vnilink::fuzzing` names what each feeds and the property it checks) for
 * a bounded time each, with nightly Rust. `node build/run.ts fuzz [target…]
 * [--seconds N]` runs the named targets, or all of them, for N seconds each
 * (60 by default); a crash stops the run, and cargo-fuzz leaves the input in
 * `crates/vnilink/fuzz/artifacts/<target>`. The corpus grows in
 * `crates/vnilink/fuzz/corpus/<target>` between runs; both are ignored.
 */
import { execFile } from 'node:child_process';
import { join } from 'node:path';
import { promisify } from 'node:util';
import { ProcessRunner } from './processes.ts';

export interface FuzzSelection {
	readonly targets: readonly string[];
	readonly seconds: number;
}

/** Reads `[target…] [--seconds N]`; a target the fuzz crate does not list is refused. */
export function fuzzSelection(args: readonly string[], available: readonly string[]): FuzzSelection {
	const targets: string[] = [];
	let seconds = 60;
	for (let index = 0; index < args.length; index++) {
		if (args[index] === '--seconds') {
			seconds = Number(args[++index]);
			if (!Number.isInteger(seconds) || seconds < 1) {
				throw new Error('--seconds needs a whole number of seconds, at least 1');
			}
		} else if (args[index].startsWith('-')) {
			throw new Error(`Unknown fuzz option: ${args[index]}. Options: [target…] --seconds <n>`);
		} else {
			targets.push(args[index]);
		}
	}
	const unknown = targets.filter(target => !available.includes(target));
	if (unknown.length) {
		throw new Error(`No fuzz target ${unknown.join(', ')}. Targets: ${available.join(', ')}`);
	}
	return { targets: targets.length ? targets : [...available], seconds };
}

export async function runFuzzTargets(root: string, signal: AbortSignal, args: readonly string[]): Promise<void> {
	const crate = join(root, 'crates/vnilink');
	const processes = new ProcessRunner(signal);
	try {
		const { stdout } = await promisify(execFile)('cargo', ['+nightly', 'fuzz', 'list'], { cwd: crate, signal });
		const selection = fuzzSelection(args, stdout.split('\n').map(line => line.trim()).filter(Boolean));
		for (const target of selection.targets) {
			signal.throwIfAborted();
			console.log(`Fuzzing ${target} for ${String(selection.seconds)} s`);
			await processes.run('cargo', ['+nightly', 'fuzz', 'run', target, '--', `-max_total_time=${String(selection.seconds)}`], { cwd: crate });
		}
	} finally {
		await processes.stop();
	}
}
