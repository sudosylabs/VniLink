/**
 * Runs after `npm install` and `npm ci`, after VS Code's
 * `build/npm/postinstall.ts`: prepares what the first build and launch would
 * otherwise stop to fetch, and records the inputs it prepared for so that a
 * reinstall with the same inputs skips it (`VNILINK_FORCE_INSTALL=1` forces
 * it). Each step is idempotent and best-effort: a step that fails is reported
 * with its fix and retried on the next install, and the task that needs it
 * checks again before it runs.
 *
 * - the Rust toolchain `rust-toolchain.toml` names, with its components;
 * - on Windows, Rust's GNU target, which the toolchain file cannot add for
 *   Windows alone;
 * - the branded development Electron in `.build/electron`.
 */
import { fileURLToPath } from 'node:url';
import { electronExecutable } from '../lib/electron.ts';
import { loadManifest } from '../lib/prerequisites.ts';
import { ProcessRunner } from '../lib/processes.ts';
import { isUpToDate, saveState } from './installState.ts';

const root = fileURLToPath(new URL('../../', import.meta.url));

function log(message: string): void {
	console.log(process.stdout.isTTY ? `\x1b[34m[postinstall]\x1b[0m ${message}` : `[postinstall] ${message}`);
}

async function step(name: string, fix: string, work: () => Promise<unknown>): Promise<boolean> {
	try {
		await work();
		return true;
	} catch (error) {
		console.warn(`\x1b[33m[postinstall] ${name} failed: ${error instanceof Error ? error.message : String(error)}\n  ${fix}\x1b[0m`);
		return false;
	}
}

if (!process.env.VNILINK_FORCE_INSTALL && await isUpToDate(root)) {
	log('Already prepared for these dependencies and toolchain; VNILINK_FORCE_INSTALL=1 prepares again.');
} else {
	const manifest = await loadManifest();
	const processes = new ProcessRunner();
	try {
		const results: boolean[] = [];
		// rustup reads rust-toolchain.toml from the checkout; an existing toolchain is a no-op.
		// Updating rustup itself is the user's decision, not an install's.
		results.push(await step('Installing the Rust toolchain', 'Install rustup from https://rustup.rs, then run `rustup toolchain install` here.', async () => {
			log('rustup toolchain install (rust-toolchain.toml)');
			await processes.run('rustup', ['toolchain', 'install', '--no-self-update'], { cwd: root });
		}));
		const target = manifest.platforms.win32.rustTarget;
		if (process.platform === 'win32' && target) {
			results.push(await step(`Adding the Rust ${target} target`, `Run \`rustup target add ${target}\` here.`, async () => {
				log(`rustup target add ${target}`);
				await processes.run('rustup', ['target', 'add', target], { cwd: root });
			}));
		}
		results.push(await step('Preparing the development Electron', 'The first launch prepares it again; `npm run doctor` reports what it needs.', async () => {
			await electronExecutable(root, new AbortController().signal, { sandboxed: false });
		}));
		if (results.every(Boolean)) {
			await saveState(root);
		}
	} finally {
		await processes.stop();
	}
}
