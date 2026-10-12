import { spawn } from 'node:child_process';
import { access } from 'node:fs/promises';
import { createRequire } from 'node:module';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import electron from 'electron';
import { watchApplication } from '../build/watch.mjs';

const root = fileURLToPath(new URL('../', import.meta.url));
const watchOnly = process.argv.includes('--watch-only');
const children = new Set();
let watcher;
let stopping = false;
async function stop(code = 0) {
	if (stopping) {
		return;
	}
	stopping = true;
	const exits = [...children].map(child => new Promise(resolve => {
		const timer = setTimeout(() => child.kill('SIGKILL'), 3000);
		child.once('exit', () => {
			clearTimeout(timer); resolve();
		});
		child.kill('SIGTERM');
	}));
	await Promise.all([watcher?.dispose(), ...exits]);
	process.exitCode = code;
}
function run(command, args, options = {}) {
	const child = spawn(command, args, { cwd: root, stdio: 'inherit', ...options });
	children.add(child);
	child.once('error', error => {
		children.delete(child);
		console.error('[dev]', error);
		void stop(1);
	});
	child.once('exit', code => {
		children.delete(child);
		if (!stopping) {
			void stop(code ?? 1);
		}
	});
	return child;
}
process.on('SIGINT', () => {
	void stop();
});
process.on('SIGTERM', () => {
	void stop();
});
try {
	if (!watchOnly) {
		try {
			await access(join(root, 'out/native/vnilink.node'));
		} catch {
			throw new Error('Native addon missing. Run npm run build once, then npm run dev. Rust is not watched.');
		}
	}
	watcher = await watchApplication(root);
	if (stopping) {
		await watcher.dispose();
	} else {
		const tsc = createRequire(import.meta.url).resolve('typescript/bin/tsc');
		run(process.execPath, [tsc, '--noEmit', '--watch', '--preserveWatchOutput']);
		run(process.execPath, [tsc, '-p', 'src/tsconfig.json', '--noEmit', '--watch', '--preserveWatchOutput']);
		if (!watchOnly) {
			run(electron, [root, ...process.argv.slice(2)], { env: { ...process.env, VNILINK_DEV: '1' } });
		}
	}
} catch (error) {
	console.error('[dev]', error);
	await stop(1);
}
