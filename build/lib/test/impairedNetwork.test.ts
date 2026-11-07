// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'assert';
import { suite, test } from 'node:test';
import { DEFAULT_OPTIONS, parseImpairedArguments, parseSize, resolveOptions, type ImpairedOptions, type ScenarioName } from '../measure/impairedOptions.ts';
import { observeRelay, parseSocketSample, relayUrl, type RelayCounters } from '../measure/impairedRelay.ts';
import { renderSummary, type ScenarioRun } from '../measure/impairedReport.ts';
import { conditionMismatch, expectedConditions, pathMismatch, scenarioEnvironment, type ConditionExpectation } from '../measure/impairedScenarios.ts';
import {
	blackoutArguments, creationCommands, NETEM_PROFILES, netemArguments, profileCommands, qdiscCommands, RELAY_PATH, relayPathCommands, selectProfiles,
	shellScript, statisticsDelta, topology, type LinkPath, type NetemProfile
} from '../measure/netem.ts';

function named(name: string): NetemProfile {
	const found = NETEM_PROFILES.find(item => item.name === name);
	assert.ok(found, `no profile ${name}`);
	return found;
}

/** A scenario run with no link counters, timing or error unless given. */
function scenarioRun(run: Pick<ScenarioRun, 'profile' | 'scenario' | 'status' | 'result'> & Partial<ScenarioRun>): ScenarioRun {
	return { seconds: 0, link: null, error: null, ...run };
}

suite('impaired network', () => {
	suite('profiles', () => {
		test('freezes every profile', () => {
			assert.deepStrictEqual(NETEM_PROFILES.filter(item => !Object.isFrozen(item)).map(item => item.name), []);
		});
		test('describes every profile in a sentence', () => {
			assert.deepStrictEqual(NETEM_PROFILES.filter(item => !item.description.endsWith('.')).map(item => item.name), []);
		});
		test('expands a group in place and selects each profile once', () => {
			assert.deepStrictEqual(selectProfiles(['loss-1', 'quick']).map(item => item.name), ['loss-1', 'clean', 'loss-5', 'loss-burst', 'delay-50', 'rate-5']);
		});
		test('selects a profile a group already names once', () => {
			assert.strictEqual(selectProfiles(['all', 'clean']).length, NETEM_PROFILES.length);
		});
		test('rejects an unknown profile', () => {
			assert.throws(() => selectProfiles(['loss-3']), /Unknown network profile: loss-3/);
		});
		test('requires at least one profile', () => {
			assert.throws(() => selectProfiles([]), /at least one/);
		});
	});
	suite('netem arguments', () => {
		test('state zero loss explicitly for the clean profile', () => {
			assert.deepStrictEqual(netemArguments(named('clean'), 17), ['loss', 'random', '0%', 'limit', '100000', 'seed', '17']);
		});
		test('split a loss profile\'s round trip across the two directions', () => {
			assert.deepStrictEqual(netemArguments(named('loss-1'), 17), ['delay', '10ms', 'loss', 'random', '1%', 'limit', '100000', 'seed', '17']);
		});
		test('express burst loss as a Gilbert-Elliott model with the given seed', () => {
			assert.deepStrictEqual(netemArguments(named('loss-burst'), 3), ['delay', '10ms', 'loss', 'gemodel', '0.5%', '25%', '100%', '0%', 'limit', '100000', 'seed', '3']);
		});
		test('express jitter as netem slots', () => {
			assert.deepStrictEqual(netemArguments(named('delay-50'), 17), ['delay', '25ms', 'slot', '0ms', '5ms', 'loss', 'random', '0%', 'limit', '100000', 'seed', '17']);
		});
		// 2 Mb/s for 250 ms of queue plus the 10 ms one-way delay is 44 packets.
		test('size a rate profile\'s queue to 250 ms plus the one-way delay', () => {
			assert.deepStrictEqual(netemArguments(named('rate-2'), 17), ['delay', '10ms', 'loss', 'random', '0%', 'rate', '2mbit', 'limit', '44', 'seed', '17']);
		});
		test('size the 10 Mb/s queue at 217 packets', () => {
			assert.deepStrictEqual(netemArguments(named('rate-10'), 17).slice(-4), ['limit', '217', 'seed', '17']);
		});
		test('black a profile out with total loss and a fixed seed', () => {
			assert.deepStrictEqual(blackoutArguments(named('delay-100')), ['delay', '50ms', 'slot', '0ms', '10ms', 'loss', 'random', '100%', 'limit', '100000', 'seed', '1']);
		});
		test('differ between a blackout and its profile only in loss', () => {
			const withoutLoss = (args: readonly string[]) => args.filter((_, index) => index !== 7);
			assert.deepStrictEqual(withoutLoss(netemArguments(named('delay-100'), 1)), withoutLoss(blackoutArguments(named('delay-100'))));
		});
	});
	suite('topology', () => {
		const link = topology({ host: 'a', view: 'b' });
		const commands = creationCommands(link).map(command => command.join(' '));
		const sides = [['a', 'a-veth'], ['b', 'b-veth']] as const;
		test('joins the namespaces with one veth pair', () => {
			assert.ok(commands.includes('ip link add a-veth type veth peer name b-veth'));
		});
		for (const [namespace, device] of sides) {
			test(`routes loopback in ${namespace} over its veth`, () => {
				assert.ok(commands.includes(`ip -n ${namespace} route add 127.0.0.1/32 dev ${device} table 100 scope link`));
			});
			// Locally sent packets take the veth table before the local table; received packets do not.
			test(`sends local packets in ${namespace} to the veth table before the local table`, () => {
				const outbound = commands.indexOf(`ip -n ${namespace} rule add pref 10 iif lo lookup 100`);
				const local = commands.indexOf(`ip -n ${namespace} rule add pref 20 lookup local`);
				const removed = commands.indexOf(`ip -n ${namespace} rule del pref 0`);
				assert.ok(outbound >= 0 && outbound < local && local < removed);
			});
		}
		// Profile changes start from fresh qdiscs, since netem keeps parameters a
		// replacement leaves out. A token bucket root splits GSO batches first.
		test('starts a profile change from fresh qdiscs under a token bucket', () => {
			assert.deepStrictEqual(profileCommands(link, ['limit', '5']).slice(0, 3).map(command => command.slice(6).join(' ')), [
				'a-veth root',
				'a-veth root handle 1: tbf rate 10gbit burst 2000 latency 1s',
				'a-veth parent 1:1 handle 10: netem limit 5'
			]);
		});
		test('impairs the viewer\'s direction too', () => {
			assert.ok(commands.includes('tc -n b qdisc add dev b-veth parent 1:1 handle 10: netem limit 100000'));
		});
		test('deletes no qdisc while creating the link', () => {
			assert.ok(!commands.some(command => command.includes('qdisc del')));
		});
		test('replaces the netem qdisc on both sides', () => {
			assert.deepStrictEqual(qdiscCommands(link, ['limit', '5']), [
				['tc', '-n', 'a', 'qdisc', 'replace', 'dev', 'a-veth', 'parent', '1:1', 'handle', '10:', 'netem', 'limit', '5'],
				['tc', '-n', 'b', 'qdisc', 'replace', 'dev', 'b-veth', 'parent', '1:1', 'handle', '10:', 'netem', 'limit', '5']
			]);
		});
		const relay = relayPathCommands(link, { host: 'aa:aa', view: 'bb:bb' }).map(command => command.join(' '));
		// The viewer reaches the relay from its own loopback address across the veth.
		test('routes the viewer to the relay from its own loopback address', () => {
			assert.ok(relay.includes(`ip -n b route add ${RELAY_PATH.address}/32 dev b-veth table 100 scope link src ${RELAY_PATH.viewerSource}`));
		});
		test('gives the viewer a permanent neighbour for the relay', () => {
			assert.ok(relay.includes(`ip -n b neigh replace ${RELAY_PATH.address} lladdr aa:aa dev b-veth nud permanent`));
		});
		test('routes the host back to the viewer\'s source address', () => {
			assert.ok(relay.includes(`ip -n a route add ${RELAY_PATH.viewerSource}/32 dev a-veth table 100 scope link`));
		});
		test('gives the host a permanent neighbour for the viewer', () => {
			assert.ok(relay.includes(`ip -n a neigh replace ${RELAY_PATH.viewerSource} lladdr bb:bb dev a-veth nud permanent`));
		});
		// The relay's replies to the host core stay local, ahead of the veth rule.
		test('keeps the relay\'s replies to the host core local', () => {
			assert.ok(relay.includes(`ip -n a rule add pref 5 from ${RELAY_PATH.address} to 127.0.0.1 lookup local`));
		});
		for (const [namespace, device] of sides) {
			test(`adds a classifier qdisc to the ${namespace} veth on the relay path`, () => {
				assert.ok(relay.includes(`tc -n ${namespace} qdisc add dev ${device} clsact`));
			});
			test(`drops UDP leaving ${namespace} on the relay path`, () => {
				assert.ok(relay.includes(`tc -n ${namespace} filter add dev ${device} egress protocol ip pref 1 u32 match ip protocol 17 0xff action drop`));
			});
		}
		test('names the relay by its address and port', () => {
			assert.strictEqual(relayUrl(), `http://${RELAY_PATH.address}:${String(RELAY_PATH.port)}`);
		});
		test('writes hook scripts that stop at the first failure and quote every argument', () => {
			assert.strictEqual(shellScript([['tc', 'qdisc', 'it\'s']]), '#!/bin/sh\nset -e\n\'tc\' \'qdisc\' \'it\'\\\'\'s\'\n');
		});
		test('reports link counters as differences', () => {
			const before = { a: { txPackets: 10, netemDrops: 1 } };
			const after = { a: { txPackets: 25, netemDrops: 4 }, b: { txPackets: 2 } };
			assert.deepStrictEqual(statisticsDelta(before, after), { a: { txPackets: 15, netemDrops: 3 }, b: { txPackets: 2 } });
		});
	});
	suite('path check', () => {
		const sample = (host: readonly string[], viewer: readonly string[]) => ({ host: host.map(path => ({ path })), viewer: viewer.map(path => ({ path })) });
		const result = (connected: ReturnType<typeof sample>, finished: ReturnType<typeof sample>) => ({ paths: { connected, finished } });
		test('accepts a run on the expected path', () => {
			assert.strictEqual(pathMismatch(result(sample(['relay'], ['relay']), sample(['relay'], ['relay'])), 'relay'), null);
		});
		test('accepts a path not selected yet right after connecting', () => {
			assert.strictEqual(pathMismatch(result(sample([], ['none']), sample(['direct'], ['direct'])), 'direct'), null);
		});
		test('reports a core that connected on another path', () => {
			assert.match(pathMismatch(result(sample(['relay'], ['direct']), sample(['relay'], ['relay'])), 'relay') ?? '', /viewer core's connection used the direct path when connected/);
		});
		test('reports a core without a connection on the path when finished', () => {
			assert.match(pathMismatch(result(sample(['relay'], ['relay']), sample(['relay'], ['none'])), 'relay') ?? '', /viewer core reported no relay connection when finished \(none\)/);
		});
		test('reports a core without connections', () => {
			assert.match(pathMismatch(result(sample([], []), sample([], [])), 'direct') ?? '', /host core reported no direct connection when finished \(no connections\)/);
		});
		test('reports a result without transport paths', () => {
			assert.match(pathMismatch({ cases: [] }, 'relay') ?? '', /no transport paths/);
		});
	});
	suite('options', () => {
		for (const [text, bytes] of [['4096', 4096], ['256MiB', 256 * 1024 ** 2], ['10G', 10 * 1024 ** 3]] as const) {
			test(`parses the size ${text}`, () => {
				assert.strictEqual(parseSize(text), bytes);
			});
		}
		test('rejects a fractional size', () => {
			assert.throws(() => parseSize('1.5G'), /Not a size/);
		});
		const resolved = resolveOptions({ profiles: ['rate-2'], scenarios: ['video'] });
		test('resolves profile names to profiles', () => {
			assert.deepStrictEqual(resolved.profiles.map(item => item.name), ['rate-2']);
		});
		test('fills unset options from the defaults', () => {
			assert.strictEqual(resolved.largeBytes, DEFAULT_OPTIONS.largeBytes);
		});
		test('takes the direct path by default', () => {
			assert.strictEqual(resolved.path, 'direct');
		});
		const invalid: readonly (readonly [string, Partial<ImpairedOptions>, RegExp])[] = [
			['an unknown scenario', { scenarios: ['audio'] }, /Unknown scenario: audio/],
			['an unknown driver', { driver: 'fast' }, /tight or background/],
			['an unknown viewer behaviour', { viewer: 'eager' }, /passive or request-while-corrupt/],
			['an unknown path', { path: 'turn' }, /direct or relay/],
			['a negative burst count', { bursts: -1 }, /bursts must be a whole number/],
			['a large-file size that is not a number', { largeBytes: Number.NaN }, /largeBytes/],
		];
		for (const [description, options, message] of invalid) {
			test(`rejects ${description} before any namespace exists`, () => {
				assert.throws(() => resolveOptions(options), message);
			});
		}
		test('maps task arguments to options', () => {
			const parsed = parseImpairedArguments(['--profiles', 'loss-1, quick', '--large-size', '1GiB', '--bursts', '0', '--output', '/e', '--path', 'relay']);
			assert.deepStrictEqual(parsed, { output: '/e', profiles: ['loss-1', 'quick'], largeBytes: 1024 ** 3, bursts: 0, path: 'relay' });
		});
		test('maps no arguments to no options', () => {
			assert.deepStrictEqual(parseImpairedArguments([]), {});
		});
		for (const [description, args, message] of [['an unknown option', ['--burst', '3'], /Unknown option/], ['a fractional seed', ['--seed', '1.5'], /--seed must be a whole number/], ['a positional argument', ['loss-1'], /positional/]] as const) {
			test(`rejects ${description} among the task arguments`, () => {
				assert.throws(() => parseImpairedArguments(args), message);
			});
		}
	});
	suite('scenario environment', () => {
		const link = topology({ host: 'h', view: 'v' });
		const paths = { result: '/r.json', scratch: '/s', burstStart: '/start', burstEnd: '/end' };
		const options = resolveOptions({});
		const transfer = scenarioEnvironment('transfer', options, paths, link);
		const video = scenarioEnvironment('video', options, paths, link);
		test('names the namespaces to a scenario', () => {
			assert.strictEqual(transfer.VNILINK_IMPAIRED_HOST_NS, 'h');
		});
		test('gives the transfer its large-file size', () => {
			assert.strictEqual(transfer.VNILINK_IMPAIRED_LARGE_BYTES, String(1024 ** 3));
		});
		test('gives the transfer no burst hooks', () => {
			assert.strictEqual(transfer.VNILINK_IMPAIRED_BURST_START, undefined);
		});
		test('gives the video its burst hooks', () => {
			assert.strictEqual(video.VNILINK_IMPAIRED_BURST_START, '/start');
		});
		test('gives the video its viewer behaviour', () => {
			assert.strictEqual(video.VNILINK_IMPAIRED_VIEWER, 'passive');
		});
		test('gives the video no transfer sizes', () => {
			assert.strictEqual(video.VNILINK_IMPAIRED_SMALL_FILES, undefined);
			assert.strictEqual(video.VNILINK_IMPAIRED_LARGE_BYTES, undefined);
		});
		// The combined streams move the large-file case beside the video, either way.
		for (const name of ['combined', 'combined-download'] as const) {
			test(`gives ${name} the video settings and the large-file size`, () => {
				assert.deepStrictEqual(scenarioEnvironment(name, options, paths, link), { ...video, VNILINK_IMPAIRED_LARGE_BYTES: String(1024 ** 3) });
			});
		}
		// Only relay mode names a relay; direct cores keep their dead one.
		test('names no relay on the direct path', () => {
			assert.strictEqual(transfer.VNILINK_IMPAIRED_RELAY_URL, undefined);
		});
		const relayed = resolveOptions({ path: 'relay' });
		for (const name of ['transfer', 'video', 'combined', 'combined-download', 'stills'] satisfies ScenarioName[]) {
			test(`names the relay to ${name} on the relay path`, () => {
				assert.strictEqual(scenarioEnvironment(name, relayed, paths, link).VNILINK_IMPAIRED_RELAY_URL, relayUrl());
			});
		}
	});
	suite('summary', () => {
		const summary = renderSummary({
			started: '2026-10-20T00:00:00.000Z',
			commit: 'abc',
			runs: [
				scenarioRun({ profile: 'loss-1', scenario: 'transfer', status: 'passed', result: { cases: [{ case: 'small', outcome: 'complete', elapsed_ms: 2500, mb_per_s: 1.6384, files_per_s: 400 }, { case: 'large', outcome: 'refused', error: 'the transfer exceeds its limits' }] } }),
				scenarioRun({ profile: 'loss-1', scenario: 'video', status: 'passed', result: { delivery_ratio: 0.9, clean_ratio: 0.5, keyframes_produced: { open: 1, forced: 2, periodic: 7 }, bursts: [{ time_to_picture_ms: 300 }, { time_to_picture_ms: 1200 }, { time_to_picture_ms: null }], unrecovered_bursts: 1, latency_ms: { p50: 12.34, p95: 40 }, engine: { host: { pipelines: [{ keyframes_sent: 9 }] }, viewer: { pipelines: [{ lost_gap: 3, lost_expired: 1, keyframes_requested: 4 }] } } } }),
				scenarioRun({ profile: 'rate-2', scenario: 'video', status: 'failed', error: 'cargo exited (101)', result: null }),
				scenarioRun({ profile: 'loss-1', scenario: 'combined', status: 'passed', result: { delivery_ratio: 0.5, clean_ratio: 0.4, keyframes_produced: { open: 1, forced: 0, periodic: 2 }, bursts: [{ time_to_picture_ms: 700 }], unrecovered_bursts: 0, latency_ms: { p50: 80, p95: 2500 }, longest_freeze_ms: 4210.6, session_interruptions: 1, timeline: [{ rtt_ms: 40 }, { rtt_ms: 1800 }, { rtt_ms: 2000 }], transfer: { direction: 'upload', outcome: 'timed-out', elapsed_ms: 30000, done_bytes: 15e6, mb_per_s: 0.5 } } }),
				scenarioRun({ profile: 'rate-5', scenario: 'combined-download', status: 'failed', error: 'cargo exited (101)', result: null })
			]
		});
		test('states that the evidence is from a Linux container only', () => {
			assert.match(summary, /Linux container evidence only/);
		});
		test('reports a completed transfer case with its rates', () => {
			assert.match(summary, /\| loss-1 \| small \| complete \| 2\.5 \| 1\.64 \| 400\.0 \|/);
		});
		test('reports a refused transfer case with its reason', () => {
			assert.match(summary, /\| loss-1 \| large \| refused: the transfer exceeds its limits \|/);
		});
		test('reports video delivery, keyframes, losses, recovery and latency', () => {
			assert.match(summary, /\| loss-1 \| 90\.0% \| 50\.0% \| 1\/2\/7 \| 4\/4\/9 \| 300\/1200 \| 1\/3 \| 12\.3\/40\.0 \|/);
		});
		test('lists each failed run with its error', () => {
			assert.match(summary, /- rate-2 video: cargo exited \(101\)/);
		});
		test('heads desktop video beside a large transfer', () => {
			assert.match(summary, /## Desktop video beside a large transfer/);
		});
		test('reports the combined run\'s video, freeze, interruptions, round trip and transfer', () => {
			assert.match(summary, /\| loss-1 \| upload \| 50\.0% \| 700\/700 \| 0\/1 \| 80\.0\/2500\.0 \| 4211 \| 1 \| 1800 \| timed-out, 30\.0 s \| 0\.50 \|/);
		});
		test('reports a failed combined download', () => {
			assert.match(summary, /\| rate-5 \| download \| failed \|/);
		});
		test('names no relay path for a direct run', () => {
			assert.doesNotMatch(summary, /Path: relay/);
		});
		test('says a relay run covers neither a public relay nor an internet path', () => {
			assert.match(renderSummary({ started: 'now', commit: 'abc', path: 'relay', runs: [] }), /Path: relay\. .*not a public relay or an internet path/);
		});
		const half = (first: { p50: number; p95: number; max: number }, host: { stills_sent: number; still_bytes: number; stills_dropped: number } | null) => ({
			changes: 7,
			changes_unshown: host ? 0 : 1,
			first_picture_latency_ms: first,
			latency_ms: { p50: 5, p95: 20 },
			stills_received: host ? 6 : 0,
			still_after_ms: host ? { p50: 2100, max: 2400 } : undefined,
			host,
		});
		const stills = renderSummary({
			started: 'now',
			commit: 'abc',
			runs: [
				scenarioRun({ profile: 'rate-2', scenario: 'stills', status: 'passed', result: { off: half({ p50: 10, p95: 30, max: 40 }, null), on: half({ p50: 12, p95: 90, max: 180 }, { stills_sent: 6, still_bytes: 1_200_000, stills_dropped: 2 }), paths: {} } }),
				scenarioRun({ profile: 'loss-1', scenario: 'stills', status: 'failed', error: 'cargo exited (101)', result: null }),
			],
		});
		test('heads the still pictures', () => {
			assert.match(stills, /## Still pictures/);
		});
		test('sets a change\'s wait with still pictures beside its wait without', () => {
			assert.match(stills, /\| rate-2 \| 10\.0\/30\.0\/40\.0 \| 12\.0\/90\.0\/180\.0 \| 20\.0\/20\.0 \| 6\/7\/7\/7 \| 6\/2\/6 \| 200 \| 2100\/2400 \|/);
		});
		test('reports a failed stills run with empty cells', () => {
			assert.match(stills, /\| loss-1 \| failed \| – \| – \| – \| – \| – \| – \|$/m);
		});
	});
	suite('relay observation', () => {
		const first = parseSocketSample('vnl-view', [
			'ESTAB 0      4194304 127.0.0.3:41000 127.0.0.2:3340',
			'\t cubic wscale:7,7 rto:1200 rtt:950.5/40 mss:1448 cwnd:4 bytes_sent:9 retrans:1/37 notsent:3000000',
			'ESTAB 0      0       127.0.0.3:41002 127.0.0.2:3340',
			'\t cubic rtt:20/5 cwnd:10',
		].join('\n'));
		const later = parseSocketSample('vnl-view', 'ESTAB 0 1000 127.0.0.3:41000 127.0.0.2:3340\n\t rtt:30/5 cwnd:20 retrans:0/40');
		const base: RelayCounters = { bytes_recv: 0, bytes_sent: 0, frames_recv: 10, frames_sent: 5, frames_dropped: 1, accepts: 2, disconnects: 0 };
		const after: RelayCounters = { ...base, frames_recv: 110, frames_sent: 60, frames_dropped: 3, accepts: 5, disconnects: 3 };
		const observed = observeRelay(base, after, [...first, ...later]);
		test('parses a socket\'s send queue, round trip, window and retransmissions', () => {
			assert.deepStrictEqual(first[0], { namespace: 'vnl-view', local: '127.0.0.3:41000', peer: '127.0.0.2:3340', sendQueue: 4194304, rttMs: 950.5, cwnd: 4, retransmitted: 37 });
		});
		test('leaves retransmissions unknown for a socket that reports none', () => {
			assert.strictEqual(first[1].retransmitted, null);
		});
		test('counts the frames the relay queue discarded', () => {
			assert.strictEqual(observed.queueDiscarded, 100 - 55 - 2);
		});
		test('reports relay counters as differences', () => {
			assert.strictEqual(observed.counters?.accepts, 3);
		});
		test('keeps each socket\'s peaks and latest retransmissions', () => {
			assert.deepStrictEqual(observed.sockets[0], { namespace: 'vnl-view', local: '127.0.0.3:41000', peer: '127.0.0.2:3340', peakSendQueue: 4194304, peakRttMs: 950.5, retransmitted: 40 });
		});
		test('leaves discards unknown without counters from before the run', () => {
			assert.strictEqual(observeRelay(null, base, []).queueDiscarded, null);
		});
		test('reports relay frames, connections and the socket peak in the summary', () => {
			const run = scenarioRun({ profile: 'loss-1', scenario: 'transfer', status: 'passed', result: null, relay: observed });
			const summary = renderSummary({ started: 'now', commit: 'abc', path: 'relay', runs: [run] });
			assert.match(summary, /\| loss-1 \| transfer \| 100\/55 \| 2\/43 \| 3\/3 \| 4\.19 MB, 951 ms \| – \|/);
		});
	});
	suite('conditions', () => {
		const expectations: readonly (readonly [string, string, LinkPath, number, number, ConditionExpectation])[] = [
			['no dominant condition on a clean link', 'clean', 'direct', 250, 5, { dominant: null, seen: [] }],
			// TCP repairs a relayed leg's loss.
			['the relay backlog to dominate relayed loss', 'loss-5', 'relay', 250, 5, { dominant: 'relay.backlog', seen: ['relay.path'] }],
			['link loss to dominate direct loss', 'loss-5', 'direct', 250, 5, { dominant: 'link.loss', seen: [] }],
			// Long blackouts dominate by construction.
			['only the blackout under long blackouts', 'rate-2', 'direct', 3000, 5, { seen: ['link.blackout'] }],
			['congestion on the slowest rate', 'rate-2', 'direct', 250, 5, { seen: ['link.congestion'] }],
			['no blackout without bursts', 'rate-5', 'direct', 3000, 0, { seen: [] }],
			// Under the controller's loss threshold.
			['nothing of 1% loss', 'loss-1', 'direct', 250, 5, { seen: [] }],
			['link loss to dominate burst loss', 'loss-burst', 'direct', 250, 5, { dominant: 'link.loss', seen: [] }],
		];
		for (const [description, profile, path, burstMs, bursts, expected] of expectations) {
			test(`expects ${description}`, () => {
				assert.deepStrictEqual(expectedConditions(profile, path, burstMs, bursts), expected);
			});
		}
		const result = (dominant: string | null, seen: readonly string[]) => ({ conditions: { dominant, seen } });
		test('accepts the expected dominant condition', () => {
			assert.strictEqual(conditionMismatch(result('link.loss', ['link.loss']), { dominant: 'link.loss', seen: [] }), null);
		});
		test('says which dominant condition the viewer held instead', () => {
			assert.strictEqual(conditionMismatch(result(null, []), { dominant: 'link.loss', seen: [] }), 'the dominant condition was none, not link.loss');
		});
		test('names a condition the viewer never held', () => {
			assert.strictEqual(conditionMismatch(result(null, ['relay.path']), { dominant: null, seen: ['relay.path', 'link.blackout'] }), 'the viewer never held link.blackout');
		});
		test('accepts any dominant condition when none is expected', () => {
			assert.strictEqual(conditionMismatch(result('encoder.slow', []), { seen: [] }), null);
		});
		test('reports a scenario without conditions', () => {
			assert.strictEqual(conditionMismatch({}, { seen: [] }), 'the scenario reported no conditions');
		});
	});
});
