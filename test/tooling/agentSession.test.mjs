// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'node:assert/strict';
import test from 'node:test';
import { mkdtemp, readFile, rm, stat, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { BRIDGE_PROTOCOL, openAgentSession } from '../../build/lib/remote.ts';
import { agentLatencyVariables, agentSideTakes, invitationKey, rendezvousFile } from '../../build/lib/latency.ts';
import { ACROSS_SHARING, assertSeenOnly, readRequest } from '../../test/latency/across.ts';

/** A bridge that prints `lines` first, then answers each command with `answer(command)`; with no answer it ends. */
async function fakeBridge(directory, lines, answer) {
	const script = join(directory, `bridge-${String(Math.random()).slice(2)}.mjs`);
	await writeFile(script, `
import { createInterface } from 'node:readline';
for (const line of ${JSON.stringify(lines)}) { console.log(line); }
const answer = ${answer ?? 'undefined'};
if (!answer) { process.exit(0); }
for await (const line of createInterface({ input: process.stdin })) { console.log(JSON.stringify(answer(JSON.parse(line)))); }
`);
	return { bridge: { command: process.execPath, args: [script] } };
}

const peer = { file: '/nowhere/agent.vnl', check: '123' };

test('the agent session speaks the bridge\'s lines: pairing, answers and refusals', async () => {
	const directory = await mkdtemp(join(tmpdir(), 'vnilink-bridge-'));
	const signal = new AbortController().signal;
	try {
		const approve = JSON.stringify({ event: 'approve', digits: '456' });
		const ready = JSON.stringify({ event: 'ready', platform: { os: 'linux', arch: 'x86_64', profile: 'release' }, path: 'direct', rtt_ms: 3 });
		const session = await openAgentSession('/root', peer, signal, await fakeBridge(directory, [approve, ready], `command => command.op === 'start'
			? { ok: false, error: 'the agent has no scenario "fax"' }
			: { ok: true, files: [{ name: 'host.json', data: Buffer.from('{}').toString('base64') }], running: true }`));
		assert.deepEqual(session.platform, { os: 'linux', arch: 'x86_64', profile: 'release' });
		await assert.rejects(session.start('fax'), /The agent refused start: the agent has no scenario "fax"/);
		const polled = await session.poll();
		assert.deepEqual(polled.files.map(file => [file.name, file.data.toString()]), [['host.json', '{}']], 'a refusal does not shift later answers');
		assert.equal(polled.running, true);
		await session.close();
		const failed = JSON.stringify({ event: 'failed', error: 'the digits did not match' });
		await assert.rejects(openAgentSession('/root', peer, signal, await fakeBridge(directory, [approve, failed])), /could not be paired with: the digits did not match/);
		await assert.rejects(openAgentSession('/root', peer, signal, await fakeBridge(directory, [JSON.stringify({ event: 'failed', error: 'no file' })])), /could not be paired with: no file/);
		await assert.rejects(openAgentSession('/root', peer, signal, await fakeBridge(directory, ['type 456 on the agent\'s terminal'])), error => error.code === BRIDGE_PROTOCOL);
		await assert.rejects(openAgentSession('/root', peer, signal, await fakeBridge(directory, [])), error => error.code === BRIDGE_PROTOCOL);
	} finally {
		await rm(directory, { recursive: true, force: true });
	}
});

test('a latency host across two computers runs no program the rendezvous names and is only seen', async () => {
	const directory = await mkdtemp(join(tmpdir(), 'vnilink-read-'));
	try {
		const marker = join(directory, 'ran');
		const reader = join(directory, 'reader.sh');
		await writeFile(reader, `#!/bin/sh\ntouch ${JSON.stringify(marker)}\n`, { mode: 0o755 });
		for (const value of [{ seconds: 20, reader, samples: 's', grant: 'g' }, { seconds: 20, reader }, { seconds: 20, grant: 'g' }]) {
			assert.throws(() => readRequest(value, true), /runs no reader/, JSON.stringify(value));
		}
		await assert.rejects(stat(marker), 'nothing ran');
		assert.deepEqual(readRequest({ seconds: 20 }, true), { seconds: 20, reader: undefined });
		for (const seconds of [0, 601, 1.5, '20', undefined]) {
			assert.throws(() => readRequest({ seconds }, true), /whole number of seconds/, String(seconds));
		}
		// On one computer the runner names the reader it built.
		assert.deepEqual(readRequest({ seconds: 5, reader, samples: 's', grant: 'g' }, false), { seconds: 5, reader: { path: reader, samples: 's', grant: 'g' } });
		assert.throws(() => readRequest({ seconds: 5, reader }, false), /together/);
		assert.deepEqual(ACROSS_SHARING, { view: true, control: false });
		assertSeenOnly({ view: true, control: false, files: false });
		for (const policy of [{ view: true, control: true }, { view: false, control: false }, undefined]) {
			assert.throws(() => assertSeenOnly(policy), /seen and not controlled/, JSON.stringify(policy));
		}
	} finally {
		await rm(directory, { recursive: true, force: true });
	}
});

test('what a side on the agent takes is what the test kit\'s agent accepts', async () => {
	const scenario = await readFile(new URL('../../crates/vnilink-testkit/src/peer/scenarios/launch.rs', import.meta.url), 'utf8');
	for (const [role, names] of Object.entries(agentSideTakes)) {
		const side = role === 'host' ? 'LatencyHost' : 'LatencyViewer';
		for (const name of names) {
			assert.match(scenario, new RegExp(`Side::(${side}|LatencyHost \\| Side::LatencyViewer), "${name}\\.json"`), `the agent's ${role} takes ${name}`);
		}
	}
	// The same variables in the same ranges on both sides; across and hold are never the suite's.
	const ranges = [...scenario.matchAll(/\("(VNILINK_[A-Z_]+)", (\d[\d_]*)\.\.=(\d[\d_]*)\)/g)].map(match => [match[1], [Number(match[2].replaceAll('_', '')), Number(match[3].replaceAll('_', ''))]]);
	assert.deepEqual(ranges, Object.entries(agentLatencyVariables).map(([name, range]) => [name, [...range]]));
	for (const name of ['VNILINK_LATENCY_HOLD_MS', 'VNILINK_LATENCY_ACROSS']) {
		assert.ok(!(name in agentLatencyVariables), `${name} is not the suite's to set`);
	}
	for (const name of ['host.json', 'read-done.json']) {
		assert.ok(rendezvousFile(name), name);
	}
	for (const name of ['../x.json', 'a/b.json', 'NUL.json', 'x.txt', '.json']) {
		assert.ok(!rendezvousFile(name), name);
	}
	assert.equal(invitationKey(`vnl1.${'AB'.repeat(32)}${'00'.repeat(60)}`), 'ab'.repeat(32));
	for (const token of ['vnl1.zz', 'vnl2.' + 'ab'.repeat(40), 42, undefined]) {
		assert.equal(invitationKey(token), undefined, String(token));
	}
});

test('a side runs only a script built from the sources the agent\'s revision covers', async () => {
	const { assertBundleFresh, bundleOptions } = await import('../../build/lib/bundles.ts');
	const { build } = await import('esbuild');
	const root = fileURLToPath(new URL('../../', import.meta.url));
	const directory = await mkdtemp(join(tmpdir(), 'vnilink-bundle-'));
	try {
		await assert.rejects(assertBundleFresh(root, 'latency.cjs', directory), /missing or was not built from these sources/, 'none built');
		const options = bundleOptions(root, { outputDirectory: directory }).find(candidate => candidate.outfile === join(directory, 'latency.cjs'));
		await build({ ...options, logLevel: 'silent' });
		await assertBundleFresh(root, 'latency.cjs', directory);
		await writeFile(join(directory, 'latency.cjs'), `${await readFile(join(directory, 'latency.cjs'), 'utf8')}\n// another commit\n`);
		await assert.rejects(assertBundleFresh(root, 'latency.cjs', directory), /not built from these sources/, 'a changed script');
	} finally {
		await rm(directory, { recursive: true, force: true });
	}
});
