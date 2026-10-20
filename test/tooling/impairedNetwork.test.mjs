import assert from 'node:assert/strict';
import test from 'node:test';
import { NETEM_PROFILES, blackoutArguments, creationCommands, netemArguments, profileCommands, qdiscCommands, selectProfiles, shellScript, statisticsDelta, topology } from '../../build/lib/netem.ts';
import { DEFAULT_OPTIONS, parseImpairedArguments, parseSize, renderSummary, resolveOptions, scenarioEnvironment } from '../../build/lib/impaired.ts';

const named = name => NETEM_PROFILES.find(item => item.name === name);

test('the profile catalog covers the required loss, delay and rate axes', () => {
	assert.deepEqual(NETEM_PROFILES.map(item => item.name), ['clean', 'loss-0.5', 'loss-1', 'loss-2', 'loss-5', 'loss-burst', 'delay-20', 'delay-50', 'delay-100', 'rate-2', 'rate-5', 'rate-10']);
	for (const item of NETEM_PROFILES) {
		assert.ok(Object.isFrozen(item));
		assert.ok(item.description.endsWith('.'));
	}
});

test('netem arguments split the round trip and keep loss explicit and repeatable', () => {
	assert.deepEqual(netemArguments(named('clean'), 17), ['loss', 'random', '0%', 'limit', '100000', 'seed', '17']);
	assert.deepEqual(netemArguments(named('loss-1'), 17), ['delay', '10ms', 'loss', 'random', '1%', 'limit', '100000', 'seed', '17']);
	assert.deepEqual(netemArguments(named('loss-burst'), 3), ['delay', '10ms', 'loss', 'gemodel', '0.5%', '25%', '100%', '0%', 'limit', '100000', 'seed', '3']);
	assert.deepEqual(netemArguments(named('delay-50'), 17), ['delay', '25ms', 'slot', '0ms', '5ms', 'loss', 'random', '0%', 'limit', '100000', 'seed', '17']);
	// 2 Mb/s for 250 ms of queue plus the 10 ms one-way delay is 44 packets.
	assert.deepEqual(netemArguments(named('rate-2'), 17), ['delay', '10ms', 'loss', 'random', '0%', 'rate', '2mbit', 'limit', '44', 'seed', '17']);
	assert.deepEqual(netemArguments(named('rate-10'), 17).slice(-4), ['limit', '217', 'seed', '17']);
	// A blackout differs from its profile only in loss.
	const blackout = blackoutArguments(named('delay-100'));
	assert.deepEqual(blackout, ['delay', '50ms', 'slot', '0ms', '10ms', 'loss', 'random', '100%', 'limit', '100000', 'seed', '1']);
	assert.deepEqual(netemArguments(named('delay-100'), 1).filter((_, index) => index !== 7), blackout.filter((_, index) => index !== 7));
});

test('profile selection expands groups once and rejects unknown names', () => {
	assert.deepEqual(selectProfiles(['loss-1', 'quick']).map(item => item.name), ['loss-1', 'clean', 'loss-5', 'loss-burst', 'delay-50', 'rate-5']);
	assert.equal(selectProfiles(['all', 'clean']).length, NETEM_PROFILES.length);
	assert.throws(() => selectProfiles(['loss-3']), /Unknown network profile: loss-3/);
	assert.throws(() => selectProfiles([]), /at least one/);
});

test('the topology routes loopback over the veth and impairs both directions', () => {
	const link = topology({ host: 'a', view: 'b' });
	const commands = creationCommands(link).map(command => command.join(' '));
	assert.ok(commands.includes('ip link add a-veth type veth peer name b-veth'));
	for (const side of [['a', 'a-veth'], ['b', 'b-veth']]) {
		const [namespace, device] = side;
		assert.ok(commands.includes(`ip -n ${namespace} route add 127.0.0.1/32 dev ${device} table 100 scope link`));
		// Locally sent packets take the veth table before the local table; received packets do not.
		const outbound = commands.indexOf(`ip -n ${namespace} rule add pref 10 iif lo lookup 100`);
		const local = commands.indexOf(`ip -n ${namespace} rule add pref 20 lookup local`);
		const removed = commands.indexOf(`ip -n ${namespace} rule del pref 0`);
		assert.ok(outbound >= 0 && outbound < local && local < removed);
	}
	// Profile changes start from fresh qdiscs, since netem keeps parameters a
	// replacement leaves out. A token bucket root splits GSO batches first.
	assert.deepEqual(profileCommands(link, ['limit', '5']).slice(0, 3).map(command => command.slice(6).join(' ')), [
		'a-veth root',
		'a-veth root handle 1: tbf rate 10gbit burst 2000 latency 1s',
		'a-veth parent 1:1 handle 10: netem limit 5'
	]);
	assert.ok(commands.includes('tc -n b qdisc add dev b-veth parent 1:1 handle 10: netem limit 100000'));
	assert.ok(!commands.some(command => command.includes('qdisc del')));
	assert.deepEqual(qdiscCommands(link, ['limit', '5']), [
		['tc', '-n', 'a', 'qdisc', 'replace', 'dev', 'a-veth', 'parent', '1:1', 'handle', '10:', 'netem', 'limit', '5'],
		['tc', '-n', 'b', 'qdisc', 'replace', 'dev', 'b-veth', 'parent', '1:1', 'handle', '10:', 'netem', 'limit', '5']
	]);
});

test('hook scripts stop at the first failure and quote every argument', () => {
	assert.equal(shellScript([['tc', 'qdisc', 'it\'s']]), '#!/bin/sh\nset -e\n\'tc\' \'qdisc\' \'it\'\\\'\'s\'\n');
});

test('link counters are reported as differences', () => {
	const before = { a: { txPackets: 10, netemDrops: 1 } };
	const after = { a: { txPackets: 25, netemDrops: 4 }, b: { txPackets: 2 } };
	assert.deepEqual(statisticsDelta(before, after), { a: { txPackets: 15, netemDrops: 3 }, b: { txPackets: 2 } });
});

test('options are validated before any namespace exists', () => {
	assert.equal(parseSize('4096'), 4096);
	assert.equal(parseSize('256MiB'), 256 * 1024 ** 2);
	assert.equal(parseSize('10G'), 10 * 1024 ** 3);
	assert.throws(() => parseSize('1.5G'), /Not a size/);
	const resolved = resolveOptions({ profiles: ['rate-2'], scenarios: ['video'] });
	assert.deepEqual(resolved.profiles.map(item => item.name), ['rate-2']);
	assert.equal(resolved.largeBytes, DEFAULT_OPTIONS.largeBytes);
	assert.throws(() => resolveOptions({ scenarios: ['audio'] }), /Unknown scenario: audio/);
	assert.throws(() => resolveOptions({ driver: 'fast' }), /tight or background/);
	assert.throws(() => resolveOptions({ viewer: 'eager' }), /passive or request-while-corrupt/);
	assert.throws(() => resolveOptions({ bursts: -1 }), /bursts must be a whole number/);
	assert.throws(() => resolveOptions({ largeBytes: Number.NaN }), /largeBytes/);
});

test('task arguments map to options and reject what the harness does not know', () => {
	const parsed = parseImpairedArguments(['--profiles', 'loss-1, quick', '--large-size', '1GiB', '--bursts', '0', '--output', '/e']);
	assert.deepEqual(parsed, { output: '/e', profiles: ['loss-1', 'quick'], largeBytes: 1024 ** 3, bursts: 0 });
	assert.deepEqual(parseImpairedArguments([]), {});
	assert.throws(() => parseImpairedArguments(['--burst', '3']), /Unknown option/);
	assert.throws(() => parseImpairedArguments(['--seed', '1.5']), /--seed must be a whole number/);
	assert.throws(() => parseImpairedArguments(['loss-1']), /positional/);
});

test('each scenario receives only its own settings', () => {
	const link = topology({ host: 'h', view: 'v' });
	const paths = { result: '/r.json', scratch: '/s', burstStart: '/start', burstEnd: '/end' };
	const options = resolveOptions({});
	const transfer = scenarioEnvironment('transfer', options, paths, link);
	assert.equal(transfer.VNILINK_IMPAIRED_HOST_NS, 'h');
	assert.equal(transfer.VNILINK_IMPAIRED_LARGE_BYTES, String(1024 ** 3));
	assert.equal(transfer.VNILINK_IMPAIRED_BURST_START, undefined);
	const video = scenarioEnvironment('video', options, paths, link);
	assert.equal(video.VNILINK_IMPAIRED_BURST_START, '/start');
	assert.equal(video.VNILINK_IMPAIRED_VIEWER, 'passive');
	assert.equal(video.VNILINK_IMPAIRED_SMALL_FILES, undefined);
});

test('the summary reports refusals, recovery and failed runs', () => {
	const summary = renderSummary({
		started: '2026-10-20T00:00:00.000Z',
		commit: 'abc',
		runs: [
			{ profile: 'loss-1', scenario: 'transfer', status: 'passed', result: { cases: [{ case: 'small', outcome: 'complete', elapsed_ms: 2500, mb_per_s: 1.6384, files_per_s: 400 }, { case: 'large', outcome: 'refused', error: 'the transfer exceeds its limits' }] } },
			{ profile: 'loss-1', scenario: 'video', status: 'passed', result: { delivery_ratio: 0.9, clean_ratio: 0.5, keyframes_produced: { open: 1, forced: 2, periodic: 7 }, bursts: [{ time_to_picture_ms: 300 }, { time_to_picture_ms: 1200 }, { time_to_picture_ms: null }], unrecovered_bursts: 1, latency_ms: { p50: 12.34, p95: 40 }, engine: { host: { pipelines: [{ keyframes_sent: 9 }] }, viewer: { pipelines: [{ lost_gap: 3, lost_expired: 1, keyframes_requested: 4 }] } } } },
			{ profile: 'rate-2', scenario: 'video', status: 'failed', error: 'cargo exited (101)', result: null }
		]
	});
	assert.match(summary, /Linux container evidence only/);
	assert.match(summary, /\| loss-1 \| small \| complete \| 2\.5 \| 1\.64 \| 400\.0 \|/);
	assert.match(summary, /\| loss-1 \| large \| refused: the transfer exceeds its limits \|/);
	assert.match(summary, /\| loss-1 \| 90\.0% \| 50\.0% \| 1\/2\/7 \| 4\/4\/9 \| 300\/1200 \| 1\/3 \| 12\.3\/40\.0 \|/);
	assert.match(summary, /- rate-2 video: cargo exited \(101\)/);
});
