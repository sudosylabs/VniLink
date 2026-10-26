import assert from 'node:assert/strict';
import test from 'node:test';
import { mkdtemp, mkdir, readFile, writeFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { define, execute, parallel, series } from '../../build/lib/task.ts';
import { ProcessRunner } from '../../build/lib/processes.ts';
import { assertNativeFresh, compileNative, stageNative } from '../../build/lib/native.ts';
import { agentSides, launchAgentSide, selectPeer, selectServices, testkitServices } from '../../build/lib/test.ts';

test('series runs in order, parallel settles every task, and failures stop later steps', async () => {
	const events = [];
	const step = name => define(name, async () => {
		events.push(name);
	});
	const signal = new AbortController().signal;
	await execute(define('all', series(step('first'), parallel(step('left'), step('right')), step('last'))), signal);
	assert.equal(events[0], 'first');
	assert.deepEqual(events.slice(1, 3).sort(), ['left', 'right']);
	assert.equal(events.at(-1), 'last');
	let downstream = false;
	let sibling = false;
	const failing = define('bad', async () => {
		throw new Error('failed');
	});
	const slow = define('slow', async () => {
		await new Promise(resolve => setTimeout(resolve, 20));
		sibling = true;
	});
	await assert.rejects(execute(series(parallel(failing, slow), define('downstream', async () => {
		downstream = true;
	})), signal), /failed/);
	assert.equal(downstream, false);
	assert.equal(sibling, true, 'A parallel sibling was abandoned before it settled');
	const controller = new AbortController();
	controller.abort(new Error('stopped'));
	await assert.rejects(execute(series(step('never')), controller.signal), /stopped/);
	assert(!events.includes('never'));
});

test('native builds reject an addon target that Electron cannot load on this host', async () => {
	const target = process.platform === 'win32' ? 'aarch64-apple-darwin' : 'x86_64-pc-windows-gnu';
	let invoked = false;
	await assert.rejects(compileNative('/unused', {
		run: () => {
			invoked = true;
		}
	}, { profile: 'dev', target }), /current host/);
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
		const options = { profile: 'dev' };
		const out = join(root, 'out');
		const processes = { run: async () => writeFile(join(root, 'target/electron/debug', library), 'binary') };
		let built = await compileNative(root, processes, options);
		await stageNative(root, out, options, built);
		await assertNativeFresh(root, out, options);
		await writeFile(join(root, 'out/native/vnilink.node'), 'tampered');
		await assert.rejects(assertNativeFresh(root, out, options), /stale/);
		await stageNative(root, out, options, built);
		await writeFile(join(root, 'crates/lib.rs'), 'changed');
		await assert.rejects(assertNativeFresh(root, out, options), /stale/);
		await assert.rejects(stageNative(root, out, options, built), /changed during build/);
		built = await compileNative(root, processes, options);
		await stageNative(root, out, options, built);
		await writeFile(join(root, 'crates/AGENTS.md'), 'instructions');
		await assertNativeFresh(root, out, options);
		const stamp = JSON.parse(await readFile(join(root, 'out/native/build.json'), 'utf8'));
		await writeFile(join(root, 'out/native/build.json'), JSON.stringify({ ...stamp, target: 'other' }));
		await assert.rejects(assertNativeFresh(root, out, options), /stale/);
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

test('an aborted signal stops the processes a runner owns', async () => {
	const controller = new AbortController();
	const runner = new ProcessRunner(controller.signal);
	const { child, result } = runner.start(process.execPath, ['-e', 'setInterval(() => {}, 1000)']);
	await new Promise((resolve, reject) => {
		child.once('spawn', resolve);
		child.once('error', reject);
	});
	controller.abort();
	await assert.rejects(result);
	assert.throws(() => runner.start(process.execPath, []), /closed/);
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

test('each service with a suite in the test kit runs alone, and a service without one is refused', async () => {
	const root = join(import.meta.dirname, '../..');
	const tests = await testkitServices(root, 'tests');
	const benches = await testkitServices(root, 'benches');
	assert.ok(tests.includes('files') && tests.includes('desktop'), tests.join());
	assert.ok(benches.includes('files') && benches.includes('desktop'), benches.join());
	assert.deepEqual(selectServices(tests, ['files']), ['files']);
	assert.deepEqual(selectServices(tests, []), tests);
	assert.throws(() => selectServices(tests, ['files', 'fax']), /No suite for fax\. Services: .*files/);
});

test('--peer and --peer-check name the agent a suite runs against, and nothing else is taken for a service', () => {
	assert.deepEqual(selectPeer(['files', 'desktop']), { services: ['files', 'desktop'], env: {} });
	assert.deepEqual(selectPeer(['files', '--peer', 'agent.vnl', '--peer-check', '042'], '/work'), {
		services: ['files'],
		env: { VNILINK_PEER: join('/work', 'agent.vnl'), VNILINK_PEER_CHECK: '042' },
	});
	assert.deepEqual(selectPeer(['--peer-check', '042', '--peer', '/elsewhere/agent.vnl', 'files'], '/work').env.VNILINK_PEER, resolve('/elsewhere/agent.vnl'), 'an absolute file stays where it is');
	assert.throws(() => selectPeer(['files', '--peer', 'agent.vnl']), /--peer and --peer-check go together/);
	assert.throws(() => selectPeer(['--peer-check', '042']), /--peer and --peer-check go together/);
	assert.throws(() => selectPeer(['files', '--peer']), /--peer needs a value/);
	assert.throws(() => selectPeer(['--peer', '--peer-check', '042']), /--peer needs a value/);
	assert.throws(() => selectPeer(['--peer', 'agent.vnl', '--peer-check', '42']), /three digits/);
	assert.throws(() => selectPeer(['--peer', 'agent.vnl', '--peer-check', 'abc']), /three digits/);
});

test('the agent starts only the measurement sides compiled into it, one at a time', async () => {
	const scenarios = await readFile(new URL('../../crates/vnilink-testkit/src/peer/scenarios.rs', import.meta.url), 'utf8');
	for (const side of agentSides) {
		assert.match(scenarios, new RegExp(`"${side}" =>`), `the agent has a ${side} scenario`);
	}
	const signal = new AbortController().signal;
	for (const args of [[], ['latency-sender'], ['latency-host', 'latency-viewer'], ['../latency-host']]) {
		await assert.rejects(launchAgentSide('/root', signal, args), /testkit:launch takes one of latency-host, latency-viewer, perceived-stimulus/, JSON.stringify(args));
	}
});
