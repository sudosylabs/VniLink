import assert from 'node:assert/strict';
import test from 'node:test';
import { mkdtemp, mkdir, readFile, writeFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { TaskRegistry } from '../../build/lib/tasks.mjs';
import { ProcessRunner } from '../../build/lib/processes.mjs';
import { NativeBuild } from '../../build/lib/nativeBuild.mjs';

test('task dependencies execute once per run and errors stop downstream work', async () => {
	const events = [];
	const registry = new TaskRegistry().register('shared', [], async () => events.push('shared'))
		.register('left', ['shared'], async () => events.push('left'))
		.register('right', ['shared'], async () => events.push('right'))
		.register('all', ['left', 'right'], async () => events.push('all'));
	await registry.run('all');
	assert.equal(events.filter(value => value === 'shared').length, 1);
	assert.equal(events[0], 'shared');
	assert.equal(events.at(-1), 'all');
	await registry.run('all');
	assert.equal(events.filter(value => value === 'shared').length, 2);
	let downstream = false;
	await assert.rejects(new TaskRegistry().register('bad', [], async () => {
		throw new Error('failed');
	})
		.register('downstream', ['bad'], async () => {
			downstream = true;
		}).run('downstream'), /failed/);
	assert.equal(downstream, false);
	await assert.rejects(new TaskRegistry().register('cycle', ['cycle'], async () => { }).run('cycle'), /cycle/);
	await assert.rejects(new TaskRegistry().register('root', ['missing'], async () => { }).run('root'), /Unknown task/);
});

test('native builds reject an addon target that Electron cannot load on this host', async () => {
	const target = process.platform === 'win32' ? 'aarch64-apple-darwin' : 'x86_64-pc-windows-gnu';
	let invoked = false;
	const build = new NativeBuild('/unused', {
		run: () => {
			invoked = true;
		}
	}, target);
	await assert.rejects(build.build(), /current host/);
	assert.equal(invoked, false);
});

test('native staging rejects changed sources, targets and tampered binaries', async () => {
	const root = await mkdtemp(join(tmpdir(), 'vnilink-native-identity-'));
	try {
		for (const directory of ['crates', 'vendor', '.cargo', 'out/native', 'target/electron/debug']) {
			await mkdir(join(root, directory), { recursive: true });
		}
		for (const filename of ['Cargo.toml', 'Cargo.lock', 'rust-toolchain.toml', 'crates/lib.rs']) {
			await writeFile(join(root, filename), 'initial');
		}
		const library = process.platform === 'win32' ? 'vnilink_node.dll' : process.platform === 'darwin' ? 'libvnilink_node.dylib' : 'libvnilink_node.so';
		const native = new NativeBuild(root, { run: async () => writeFile(join(root, 'target/electron/debug', library), 'binary') }, null);
		await native.build();
		await native.stage();
		await native.assertFresh();
		await writeFile(join(root, 'out/native/vnilink.node'), 'tampered');
		await assert.rejects(native.assertFresh(), /stale/);
		await native.stage();
		await writeFile(join(root, 'crates/lib.rs'), 'changed');
		await assert.rejects(native.assertFresh(), /stale/);
		await assert.rejects(native.stage(), /changed during build/);
		await native.build();
		await native.stage();
		await writeFile(join(root, 'crates/AGENTS.md'), 'instructions');
		await native.assertFresh();
		const stamp = JSON.parse(await readFile(join(root, 'out/native/build.json'), 'utf8'));
		await writeFile(join(root, 'out/native/build.json'), JSON.stringify({ ...stamp, target: 'other' }));
		await assert.rejects(native.assertFresh(), /stale/);
	} finally {
		await rm(root, { recursive: true, force: true });
	}
});

test('process ownership rejects spawn/exit failures and reaps a running worker', async () => {
	const runner = new ProcessRunner();
	await assert.rejects(runner.run('vnilink-command-does-not-exist', []));
	await assert.rejects(runner.run(process.execPath, ['-e', 'process.exit(7)']), /7/);
	const { child } = runner.start(process.execPath, ['-e', 'setInterval(() => {}, 1000)']);
	await new Promise((resolve, reject) => {
		child.once('spawn', resolve);
		child.once('error', reject);
	});
	await runner.stop();
	await runner.stop();
	assert.throws(() => runner.start(process.execPath, []), /closed/);
	assert.notEqual(child.exitCode ?? child.signalCode, null);
});

test('process tree teardown releases a descendant and closed owners cannot relaunch', async () => {
	const runner = new ProcessRunner();
	const source = `const {spawn}=require('node:child_process'); const child=spawn(process.execPath,['-e','setInterval(()=>{},1000)'],{stdio:'ignore'}); console.log(child.pid); setInterval(()=>{},1000);`;
	const { child } = runner.start(process.execPath, ['-e', source], { stdio: ['ignore', 'pipe', 'pipe'] });
	const descendant = await new Promise((resolve, reject) => {
		child.stdout.once('data', value => resolve(Number(value.toString().trim())));
		child.once('error', reject);
	});
	try {
		process.kill(descendant, 0);
		await runner.stop();
	} finally {
		await runner.stop();
	}
	const deadline = Date.now() + 5000;
	while (true) {
		try {
			process.kill(descendant, 0);
		} catch (error) {
			if (error.code === 'ESRCH') {
				break;
			}
			throw error;
		}
		assert(Date.now() < deadline, 'Descendant survived owner teardown');
		await new Promise(resolve => setTimeout(resolve, 25));
	}
});
