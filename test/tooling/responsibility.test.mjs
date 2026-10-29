import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import test from 'node:test';
import { selfResponsible } from '../../build/lib/responsibility.ts';

const root = process.cwd();

test('other systems start the executable as it is', async () => {
	for (const platform of ['win32', 'linux']) {
		assert.deepEqual(await selfResponsible(root, '/app/vnilink', ['/repo', '--flag'], AbortSignal.timeout(1000), platform), { command: '/app/vnilink', args: ['/repo', '--flag'] });
	}
});

/** Runs the launch and collects what a supervisor sees: streams and how it ended. */
function launch(command, args) {
	return new Promise((resolve, reject) => {
		const child = spawn(command, args, { stdio: ['ignore', 'pipe', 'pipe'] });
		let stdout = '';
		let stderr = '';
		child.stdout.on('data', chunk => {
			stdout += chunk;
		});
		child.stderr.on('data', chunk => {
			stderr += chunk;
		});
		child.once('error', reject);
		child.once('exit', (code, signal) => resolve({ code, signal, stdout, stderr }));
	});
}

test('on macOS the build owns its permissions and keeps its streams, exit code and stop', { skip: process.platform !== 'darwin' }, async () => {
	const { command, args } = await selfResponsible(root, '/bin/sh', ['-c', 'echo out; echo err >&2; exit 42'], AbortSignal.timeout(60000));
	assert.match(command, /\.build\/dev\/disclaim$/);
	assert.deepEqual(args, ['/bin/sh', '-c', 'echo out; echo err >&2; exit 42']);
	// 42 is the development relaunch request; it must reach the supervisor as it is.
	assert.deepEqual(await launch(command, args), { code: 42, signal: null, stdout: 'out\n', stderr: 'err\n' });
	const missing = await launch(command, ['/does/not/exist']);
	assert.equal(missing.code, 127);
	assert.match(missing.stderr, /\/does\/not\/exist/);
	// The supervisor stops the whole process group; the program goes with the launcher.
	const sleeper = spawn(command, ['/bin/sleep', '30'], { detached: true, stdio: 'ignore' });
	const stopped = new Promise(resolve => sleeper.once('exit', (code, signal) => resolve(signal)));
	await new Promise(resolve => setTimeout(resolve, 300));
	process.kill(-sleeper.pid, 'SIGTERM');
	assert.equal(await stopped, 'SIGTERM');
});
