import assert from 'node:assert/strict';
import test from 'node:test';
import { NETEM_PROFILES, RELAY_PATH, blackoutArguments, creationCommands, netemArguments, profileCommands, qdiscCommands, relayPathCommands, selectProfiles, shellScript, statisticsDelta, topology } from '../../build/lib/netem.ts';
import { DEFAULT_OPTIONS, checkoutVolume, observeRelay, parseImpairedArguments, parseSize, parseSocketSample, pathMismatch, relayUrl, renderSummary, resolveOptions, scenarioEnvironment } from '../../build/lib/impaired.ts';

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

test('relay mode reaches a relay in the host namespace and keeps UDP off the link', () => {
	const link = topology({ host: 'a', view: 'b' });
	const commands = relayPathCommands(link, { host: 'aa:aa', view: 'bb:bb' }).map(command => command.join(' '));
	// The viewer reaches the relay from its own loopback address across the veth.
	assert.ok(commands.includes(`ip -n b route add ${RELAY_PATH.address}/32 dev b-veth table 100 scope link src ${RELAY_PATH.viewerSource}`));
	assert.ok(commands.includes(`ip -n b neigh replace ${RELAY_PATH.address} lladdr aa:aa dev b-veth nud permanent`));
	assert.ok(commands.includes(`ip -n a route add ${RELAY_PATH.viewerSource}/32 dev a-veth table 100 scope link`));
	assert.ok(commands.includes(`ip -n a neigh replace ${RELAY_PATH.viewerSource} lladdr bb:bb dev a-veth nud permanent`));
	// The relay's replies to the host core stay local, ahead of the veth rule.
	assert.ok(commands.includes(`ip -n a rule add pref 5 from ${RELAY_PATH.address} to 127.0.0.1 lookup local`));
	for (const [namespace, device] of [['a', 'a-veth'], ['b', 'b-veth']]) {
		assert.ok(commands.includes(`tc -n ${namespace} qdisc add dev ${device} clsact`));
		assert.ok(commands.includes(`tc -n ${namespace} filter add dev ${device} egress protocol ip pref 1 u32 match ip protocol 17 0xff action drop`));
	}
	assert.equal(relayUrl(), `http://${RELAY_PATH.address}:${String(RELAY_PATH.port)}`);
});

test('a scenario fails unless every core reports the expected path', () => {
	const sample = (host, viewer) => ({ host: host.map(path => ({ path })), viewer: viewer.map(path => ({ path })) });
	const result = (connected, finished) => ({ paths: { connected, finished } });
	assert.equal(pathMismatch(result(sample(['relay'], ['relay']), sample(['relay'], ['relay'])), 'relay'), null);
	// A path not selected yet right after connecting is not a mismatch.
	assert.equal(pathMismatch(result(sample([], ['none']), sample(['direct'], ['direct'])), 'direct'), null);
	assert.match(pathMismatch(result(sample(['relay'], ['direct']), sample(['relay'], ['relay'])), 'relay'), /viewer core's connection used the direct path when connected/);
	assert.match(pathMismatch(result(sample(['relay'], ['relay']), sample(['relay'], ['none'])), 'relay'), /viewer core reported no relay connection when finished \(none\)/);
	assert.match(pathMismatch(result(sample([], []), sample([], [])), 'direct'), /host core reported no direct connection when finished \(no connections\)/);
	assert.match(pathMismatch({ cases: [] }, 'relay'), /no transport paths/);
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
	assert.equal(resolved.path, 'direct');
	assert.throws(() => resolveOptions({ path: 'turn' }), /direct or relay/);
	assert.throws(() => resolveOptions({ bursts: -1 }), /bursts must be a whole number/);
	assert.throws(() => resolveOptions({ largeBytes: Number.NaN }), /largeBytes/);
});

test('task arguments map to options and reject what the harness does not know', () => {
	const parsed = parseImpairedArguments(['--profiles', 'loss-1, quick', '--large-size', '1GiB', '--bursts', '0', '--output', '/e', '--path', 'relay']);
	assert.deepEqual(parsed, { output: '/e', profiles: ['loss-1', 'quick'], largeBytes: 1024 ** 3, bursts: 0, path: 'relay' });
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
	assert.equal(video.VNILINK_IMPAIRED_LARGE_BYTES, undefined);
	// The combined stream uploads the large-file case beside the video.
	const combined = scenarioEnvironment('combined', options, paths, link);
	assert.deepEqual(combined, { ...video, VNILINK_IMPAIRED_LARGE_BYTES: String(1024 ** 3) });
	// Only relay mode names a relay; direct cores keep their dead one.
	assert.equal(transfer.VNILINK_IMPAIRED_RELAY_URL, undefined);
	const relayed = resolveOptions({ path: 'relay' });
	for (const name of ['transfer', 'video', 'combined']) {
		assert.equal(scenarioEnvironment(name, relayed, paths, link).VNILINK_IMPAIRED_RELAY_URL, relayUrl());
	}
});

test('the summary reports refusals, recovery and failed runs', () => {
	const summary = renderSummary({
		started: '2026-10-20T00:00:00.000Z',
		commit: 'abc',
		runs: [
			{ profile: 'loss-1', scenario: 'transfer', status: 'passed', result: { cases: [{ case: 'small', outcome: 'complete', elapsed_ms: 2500, mb_per_s: 1.6384, files_per_s: 400 }, { case: 'large', outcome: 'refused', error: 'the transfer exceeds its limits' }] } },
			{ profile: 'loss-1', scenario: 'video', status: 'passed', result: { delivery_ratio: 0.9, clean_ratio: 0.5, keyframes_produced: { open: 1, forced: 2, periodic: 7 }, bursts: [{ time_to_picture_ms: 300 }, { time_to_picture_ms: 1200 }, { time_to_picture_ms: null }], unrecovered_bursts: 1, latency_ms: { p50: 12.34, p95: 40 }, engine: { host: { pipelines: [{ keyframes_sent: 9 }] }, viewer: { pipelines: [{ lost_gap: 3, lost_expired: 1, keyframes_requested: 4 }] } } } },
			{ profile: 'rate-2', scenario: 'video', status: 'failed', error: 'cargo exited (101)', result: null },
			{ profile: 'loss-1', scenario: 'combined', status: 'passed', result: { delivery_ratio: 0.5, clean_ratio: 0.4, keyframes_produced: { open: 1, forced: 0, periodic: 2 }, bursts: [{ time_to_picture_ms: 700 }], unrecovered_bursts: 0, latency_ms: { p50: 80, p95: 2500 }, longest_freeze_ms: 4210.6, session_interruptions: 1, upload: { outcome: 'timed-out', elapsed_ms: 30000, done_bytes: 15e6, mb_per_s: 0.5 } } }
		]
	});
	assert.match(summary, /Linux container evidence only/);
	assert.match(summary, /\| loss-1 \| small \| complete \| 2\.5 \| 1\.64 \| 400\.0 \|/);
	assert.match(summary, /\| loss-1 \| large \| refused: the transfer exceeds its limits \|/);
	assert.match(summary, /\| loss-1 \| 90\.0% \| 50\.0% \| 1\/2\/7 \| 4\/4\/9 \| 300\/1200 \| 1\/3 \| 12\.3\/40\.0 \|/);
	assert.match(summary, /- rate-2 video: cargo exited \(101\)/);
	assert.match(summary, /## Desktop video during an upload/);
	assert.match(summary, /\| loss-1 \| 50\.0% \| 700\/700 \| 0\/1 \| 80\.0\/2500\.0 \| 4211 \| 1 \| timed-out, 30\.0 s \| 0\.50 \|/);
	assert.doesNotMatch(summary, /Path: relay/);
	assert.match(renderSummary({ started: 'now', commit: 'abc', path: 'relay', runs: [] }), /Path: relay\. .*not a public relay or an internet path/);
});

test('relay counters and socket samples become per-run deltas and peaks', () => {
	const text = [
		'ESTAB 0      4194304 127.0.0.3:41000 127.0.0.2:3340',
		'\t cubic wscale:7,7 rto:1200 rtt:950.5/40 mss:1448 cwnd:4 bytes_sent:9 retrans:1/37 notsent:3000000',
		'ESTAB 0      0       127.0.0.3:41002 127.0.0.2:3340',
		'\t cubic rtt:20/5 cwnd:10',
	].join('\n');
	const first = parseSocketSample('vnl-view', text);
	assert.deepEqual(first[0], { namespace: 'vnl-view', local: '127.0.0.3:41000', peer: '127.0.0.2:3340', sendQueue: 4194304, rttMs: 950.5, cwnd: 4, retransmitted: 37 });
	assert.equal(first[1].retransmitted, null);
	const later = parseSocketSample('vnl-view', 'ESTAB 0 1000 127.0.0.3:41000 127.0.0.2:3340\n\t rtt:30/5 cwnd:20 retrans:0/40');
	const base = { bytes_recv: 0, bytes_sent: 0, frames_recv: 10, frames_sent: 5, frames_dropped: 1, accepts: 2, disconnects: 0 };
	const observed = observeRelay(base, { ...base, frames_recv: 110, frames_sent: 60, frames_dropped: 3, accepts: 5, disconnects: 3 }, [...first, ...later]);
	assert.equal(observed.queueDiscarded, 100 - 55 - 2);
	assert.deepEqual(observed.counters.accepts, 3);
	assert.deepEqual(observed.sockets[0], { namespace: 'vnl-view', local: '127.0.0.3:41000', peer: '127.0.0.2:3340', peakSendQueue: 4194304, peakRttMs: 950.5, retransmitted: 40 });
	assert.equal(observeRelay(null, base, []).queueDiscarded, null);
	const summary = renderSummary({ started: 'now', commit: 'abc', path: 'relay', runs: [{ profile: 'loss-1', scenario: 'transfer', status: 'passed', result: null, relay: observed }] });
	assert.match(summary, /\| loss-1 \| transfer \| 100\/55 \| 2\/43 \| 3\/3 \| 4\.19 MB, 951 ms \| – \|/);
});

test('each checkout caches its build in a volume of its own', () => {
	const main = checkoutVolume('vnilink-impaired-release-target', '/work/vnilink');
	const worktree = checkoutVolume('vnilink-impaired-release-target', '/work/vnilink/.claude/worktrees/one');
	assert.match(main, /^vnilink-impaired-release-target-[0-9a-f]{12}$/);
	assert.notEqual(main, worktree);
	assert.equal(main, checkoutVolume('vnilink-impaired-release-target', '/work/vnilink'));
});
