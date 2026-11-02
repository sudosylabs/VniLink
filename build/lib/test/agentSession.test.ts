// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'assert';
import { build } from 'esbuild';
import { mkdtemp, readFile, rm, stat, writeFile } from 'node:fs/promises';
import { afterEach, beforeEach, suite, test } from 'node:test';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { ACROSS_SHARING, assertSeenOnly, readRequest } from '../../../test/latency/across.ts';
import { assertBundleFresh, bundleOptions } from '../bundles.ts';
import { agentLatencyVariables, agentSideTakes, invitationKey, rendezvousFile } from '../latency.ts';
import { BRIDGE_PROTOCOL, openAgentSession, type AgentSession, type AgentSessionOptions } from '../remote.ts';

const root = join(import.meta.dirname, '../../..');

const peer = { file: '/nowhere/agent.vnl', check: '123' };

/** Whether `error` is the bridge's protocol error. */
function isProtocolError(error: unknown): boolean {
	return error instanceof Error && 'code' in error && error.code === BRIDGE_PROTOCOL;
}

suite('agent session', () => {
	let directory: string;
	beforeEach(async () => {
		directory = await mkdtemp(join(tmpdir(), 'vnilink-agent-session-'));
	});
	afterEach(async () => {
		await rm(directory, { recursive: true, force: true });
	});

	/**
	 * A bridge that prints `lines` first, then answers each command with
	 * `answer(command)`, a function in JavaScript source; with no answer it ends.
	 */
	async function fakeBridge(lines: readonly string[], answer?: string): Promise<AgentSessionOptions> {
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
	const approve = JSON.stringify({ event: 'approve', digits: '456' });
	suite('openAgentSession', () => {
		const signal = new AbortController().signal;
		suite('a paired session', () => {
			let session: AgentSession;
			beforeEach(async () => {
				const ready = JSON.stringify({ event: 'ready', platform: { os: 'linux', arch: 'x86_64', profile: 'release' }, path: 'direct', rtt_ms: 3 });
				session = await openAgentSession('/root', peer, signal, await fakeBridge([approve, ready], `command => command.op === 'start'
					? { ok: false, error: 'the agent has no scenario "fax"' }
					: { ok: true, files: [{ name: 'host.json', data: Buffer.from('{}').toString('base64') }], running: true }`));
			});
			afterEach(async () => {
				await session.close();
			});
			test('reports the platform the agent paired from', () => {
				assert.deepStrictEqual(session.platform, { os: 'linux', arch: 'x86_64', profile: 'release' });
			});
			test('rejects a command the agent refuses with its reason', async () => {
				await assert.rejects(session.start('fax'), /The agent refused start: the agent has no scenario "fax"/);
			});
			test('a refusal does not shift later answers', async () => {
				await session.start('fax').catch(() => undefined);
				const polled = await session.poll();
				assert.deepStrictEqual(polled.files.map(file => [file.name, file.data.toString()]), [['host.json', '{}']]);
			});
			test('poll reports that the scenario still runs', async () => {
				assert.strictEqual((await session.poll()).running, true);
			});
		});
		test('fails pairing with the agent\'s reason after approval', async () => {
			const failed = JSON.stringify({ event: 'failed', error: 'the digits did not match' });
			await assert.rejects(openAgentSession('/root', peer, signal, await fakeBridge([approve, failed])), /could not be paired with: the digits did not match/);
		});
		test('fails pairing with the agent\'s reason before approval', async () => {
			await assert.rejects(openAgentSession('/root', peer, signal, await fakeBridge([JSON.stringify({ event: 'failed', error: 'no file' })])), /could not be paired with: no file/);
		});
		test('a line that is not a JSON object is a protocol error', async () => {
			await assert.rejects(openAgentSession('/root', peer, signal, await fakeBridge(['type 456 on the agent\'s terminal'])), isProtocolError);
		});
		test('a bridge that ends before pairing is a protocol error', async () => {
			await assert.rejects(openAgentSession('/root', peer, signal, await fakeBridge([])), isProtocolError);
		});
	});
	suite('latency host across two computers', () => {
		const readerFields = (reader: string): readonly Readonly<Record<string, unknown>>[] => [
			{ seconds: 20, reader, samples: 's', grant: 'g' },
			{ seconds: 20, reader },
			{ seconds: 20, grant: 'g' },
		];
		for (const fields of readerFields('/reader')) {
			test(`refuses a read naming ${Object.keys(fields).filter(name => name !== 'seconds').join(', ')}`, () => {
				assert.throws(() => readRequest(fields, true), /runs no reader/);
			});
		}
		test('runs no reader a read names', async () => {
			const marker = join(directory, 'ran');
			const reader = join(directory, 'reader.sh');
			await writeFile(reader, `#!/bin/sh\ntouch ${JSON.stringify(marker)}\n`, { mode: 0o755 });
			for (const fields of readerFields(reader)) {
				assert.throws(() => readRequest(fields, true));
			}
			await assert.rejects(stat(marker));
		});
		test('takes a read of whole seconds without a reader', () => {
			assert.deepStrictEqual(readRequest({ seconds: 20 }, true), { seconds: 20, reader: undefined });
		});
		for (const seconds of [0, 601, 1.5, '20', undefined]) {
			test(`refuses ${seconds === undefined ? 'no' : JSON.stringify(seconds)} seconds`, () => {
				assert.throws(() => readRequest({ seconds }, true), /whole number of seconds/);
			});
		}
		test('on one computer a read names the reader the runner built', () => {
			assert.deepStrictEqual(readRequest({ seconds: 5, reader: '/reader', samples: 's', grant: 'g' }, false), { seconds: 5, reader: { path: '/reader', samples: 's', grant: 'g' } });
		});
		test('on one computer a read names the reader, samples and grant together', () => {
			assert.throws(() => readRequest({ seconds: 5, reader: '/reader' }, false), /together/);
		});
		test('the host shares its screen to be seen and not controlled', () => {
			assert.deepStrictEqual(ACROSS_SHARING, { view: true, control: false });
		});
		test('a policy that shares the screen to be seen only is accepted', () => {
			assertSeenOnly({ view: true, control: false, files: false });
		});
		for (const policy of [{ view: true, control: true }, { view: false, control: false }, undefined]) {
			test(`refuses the policy ${policy === undefined ? 'undefined' : JSON.stringify(policy)}`, () => {
				assert.throws(() => {
					assertSeenOnly(policy);
				}, /seen and not controlled/);
			});
		}
	});
	suite('the test kit\'s agent', () => {
		const launch = (): Promise<string> => readFile(join(root, 'crates/vnilink-testkit/src/peer/scenarios/launch.rs'), 'utf8');
		for (const [role, names] of Object.entries(agentSideTakes)) {
			const side = role === 'host' ? 'LatencyHost' : 'LatencyViewer';
			for (const name of names) {
				test(`the agent's ${role} takes ${name}`, async () => {
					assert.match(await launch(), new RegExp(`Side::(${side}|LatencyHost \\| Side::LatencyViewer), "${name}\\.json"`));
				});
			}
		}
		test('accepts the variables the suite sets, in the same ranges', async () => {
			const ranges = [...(await launch()).matchAll(/\("(VNILINK_[A-Z_]+)", (\d[\d_]*)\.\.=(\d[\d_]*)\)/g)].map(match => [match[1], [Number(match[2].replaceAll('_', '')), Number(match[3].replaceAll('_', ''))]]);
			assert.deepStrictEqual(ranges, Object.entries(agentLatencyVariables).map(([name, range]) => [name, [...range]]));
		});
		for (const name of ['VNILINK_LATENCY_HOLD_MS', 'VNILINK_LATENCY_ACROSS']) {
			test(`${name} is not the suite's to set`, () => {
				assert.ok(!(name in agentLatencyVariables));
			});
		}
		for (const name of ['host.json', 'read-done.json']) {
			test(`${name} is a rendezvous file`, () => {
				assert.ok(rendezvousFile(name));
			});
		}
		for (const name of ['../x.json', 'a/b.json', 'NUL.json', 'x.txt', '.json']) {
			test(`${name} is not a rendezvous file`, () => {
				assert.ok(!rendezvousFile(name));
			});
		}
		test('reads the key an invitation was made by, in lower case', () => {
			assert.strictEqual(invitationKey(`vnl1.${'AB'.repeat(32)}${'00'.repeat(60)}`), 'ab'.repeat(32));
		});
		for (const token of ['vnl1.zz', `vnl2.${'ab'.repeat(40)}`, 42, undefined]) {
			test(`reads no key from ${String(token)}`, () => {
				assert.strictEqual(invitationKey(token), undefined);
			});
		}
	});
	suite('side scripts', () => {
		const latencyScript = (): string => join(directory, 'latency.cjs');
		async function buildLatencyScript(): Promise<void> {
			const options = bundleOptions(root, { outputDirectory: directory }).find(candidate => candidate.outfile === latencyScript());
			assert.ok(options);
			await build({ ...options, logLevel: 'silent' });
		}
		test('refuses a script that was never built', async () => {
			await assert.rejects(assertBundleFresh(root, 'latency.cjs', directory), /missing or was not built from these sources/);
		});
		test('accepts a script built from these sources', async () => {
			await buildLatencyScript();
			await assertBundleFresh(root, 'latency.cjs', directory);
		});
		test('refuses a script that differs from these sources', async () => {
			await buildLatencyScript();
			await writeFile(latencyScript(), `${await readFile(latencyScript(), 'utf8')}\n// another commit\n`);
			await assert.rejects(assertBundleFresh(root, 'latency.cjs', directory), /not built from these sources/);
		});
	});
});
