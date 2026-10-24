import assert from 'node:assert/strict';
import test from 'node:test';
import { join, resolve } from 'node:path';
import { runInNewContext } from 'node:vm';
import { build } from 'esbuild';
import { latencyOptions, renderHoldSummary, summariseHolds } from '../../build/lib/latency.ts';

async function load(entry) {
	const output = await build({ entryPoints: [entry], bundle: true, write: false, platform: 'node', format: 'esm', tsconfig: 'tsconfig.json', external: ['electron'] });
	return import(`data:text/javascript;base64,${Buffer.from(output.outputFiles[0].text).toString('base64')}`);
}

const { cells, stimulusPage, CELLS } = await load('test/latency/stimulus.ts');

/** The strip `crates/vnilink-media/src/tests/timecode.rs` expects from the reader's layout. */
const EXPECTED = '1010101001011100001111100101101000111100000111';

test('the stimulus draws the strip the reader decodes', () => {
	const strip = cells(0xA5C3E).map(dark => dark ? '1' : '0').join('');
	assert.equal(strip, EXPECTED);
	assert.equal(strip.length, CELLS);
	assert.deepEqual(cells(1 << 20), cells(0), 'values wrap at 20 bits');
});

test('the page carries the same strip as the module', () => {
	const page = stimulusPage(30);
	const source = page.slice(page.indexOf('const VALUE_BITS'), page.indexOf('const canvas'));
	const pageCells = runInNewContext(`${source}; cells`);
	for (const value of [0, 1, 0xA5C3E, 0xFFFFF, 123456789]) {
		// Copied out of the page's realm, whose arrays have their own prototype.
		assert.deepEqual([...pageCells(value)], cells(value), `value ${String(value)}`);
	}
	assert.match(page, /const cell = 30;/);
});

test('latency options are bounded', () => {
	const options = latencyOptions('/root', ['--seconds', '5', '--settle', '1', '--output', '/out']);
	// Paths resolve the platform's way: `/out` is `C:\out` on Windows.
	assert.deepEqual(options, { seconds: 5, settleSeconds: 1, output: resolve('/out'), app: undefined, holdSeconds: 0, frameRate: undefined, remote: undefined });
	assert.equal(latencyOptions('/root', ['--hold', '6']).holdSeconds, 6);
	assert.throws(() => latencyOptions('/root', ['--hold', '61']));
	assert.throws(() => latencyOptions('/root', ['--hold', '1.5']));
	assert.equal(latencyOptions('/root', ['--app', '/x/VniLink.app']).app, '/x/VniLink.app');
	assert.throws(() => latencyOptions('/root', ['--app', '/x/VniLink']));
	assert.ok(latencyOptions('/root', []).output.startsWith(resolve('/root', '.build', 'latency')));
	assert.equal(latencyOptions('/root', ['--output', 'relative/out']).output, join(process.cwd(), 'relative/out'));
	assert.throws(() => latencyOptions('/root', ['--seconds', '0']));
	assert.throws(() => latencyOptions('/root', ['--seconds', '2.5']));
	assert.throws(() => latencyOptions('/root', ['--settle', '-1']));
	assert.throws(() => latencyOptions('/root', ['--unknown']));
});

test('a run is summarised from the screen and the viewer stages', async () => {
	const { summariseLatency, renderLatencySummary } = await import('../../build/lib/latency.ts');
	const samples = [10, 30, 20, null, 40].map(latencyMs => JSON.stringify({ shownMs: 1, codes: [], latencyMs })).join('\n');
	const stages = [
		{ at: 1, text: 'Network: about 0 ms each way\nDecoding: 2.0 ms, 5.0 ms at most for 95%\nDrawing: 0.5 ms, 1.0 ms at most for 95%' },
		{ at: 2, text: 'Decoding: 4.0 ms, 9.0 ms at most for 95%' },
		{ at: 3, text: 'Decoding: 3.0 ms, 6.0 ms at most for 95%' },
	].map(line => JSON.stringify(line)).join('\n');
	const summary = summariseLatency(samples, stages);
	assert.equal(summary.pictures, 5);
	assert.deepEqual(summary.total, { p50: 20, p95: 40, p99: 40, max: 40, count: 4 });
	assert.deepEqual(summary.stages.get('Decoding'), { p50: 3, p95: 6, windows: 3 });
	assert.deepEqual(summary.stages.get('Drawing'), { p50: 0.5, p95: 1, windows: 1 });
	assert.equal(summary.stages.has('Network'), false);
	assert.match(renderLatencySummary(summary), /\| Glass to glass \| 20 \| 40 \| 40 \| 40 \|/);
	assert.equal(summariseLatency('', '').total, undefined);
});

test('the summary shows each side\'s CPU, largest process first', async () => {
	const { summariseLatency, renderLatencySummary } = await import('../../build/lib/latency.ts');
	const text = renderLatencySummary(summariseLatency('', ''), { host: { Browser: 0.1, 'VniLink Engine': 1.25 }, viewer: { GPU: 0.5 } });
	assert.match(text, /\| host \| 1\.35 \| VniLink Engine 1\.25, Browser 0\.10 \|/);
	assert.match(text, /\| viewer \| 0\.50 \| GPU 0\.50 \|/);
	assert.doesNotMatch(renderLatencySummary(summariseLatency('', '')), /CPU while/);
});

test('the summary names the encoder the host had open and what it read', async () => {
	const { summariseLatency, renderLatencySummary } = await import('../../build/lib/latency.ts');
	const text = renderLatencySummary(summariseLatency('', ''), undefined, { encoder: 'mf-intel', hardware: true, input: 'gpu-texture' });
	assert.match(text, /Host encoder: mf-intel, hardware, input gpu-texture\./);
	assert.match(renderLatencySummary(summariseLatency('', ''), undefined, {}), /Host encoder: unknown, kind unknown, input unknown\./);
	assert.doesNotMatch(renderLatencySummary(summariseLatency('', '')), /Host encoder/);
});

test('hold mode says when each still showed and what the first change after a hold took', () => {
	const schedule = { origin: 0, moveMs: 2000, holdMs: 6000 };
	// The viewer reads its count every 100 ms; a still shows 2.3 s into the
	// first hold and 2.5 s into the second; the third brings none.
	const stills = [];
	for (let at = 1000; at <= 25000; at += 100) {
		const shown = (at >= 4300 ? 1 : 0) + (at >= 12500 ? 1 : 0);
		stills.push(JSON.stringify({ at, shown }));
	}
	// The reader: moving pictures 30 ms late, a 6 s hold without a new
	// strip, then the first change 90 ms late, then 30 ms again.
	// Windows-like: pictures keep coming while the stimulus holds, a few a second.
	const samples = [];
	let value = 100;
	let shown = 0;
	for (let index = 0; index < 20; index++) {
		samples.push(JSON.stringify({ shownMs: shown += 33, codes: [{ value: value += 16 }, { value: value - 30 }], latencyMs: 30 }));
	}
	for (let index = 0; index < 40; index++) {
		samples.push(JSON.stringify({ shownMs: shown += 150, codes: [{ value }, { value }], latencyMs: null }));
	}
	samples.push(JSON.stringify({ shownMs: shown += 33, codes: [{ value: value += 6000 }, { value: value - 90 }], latencyMs: 90 }));
	for (let index = 0; index < 5; index++) {
		samples.push(JSON.stringify({ shownMs: shown += 33, codes: [{ value: value += 16 }, { value: value - 30 }], latencyMs: 30 }));
	}
	const hold = summariseHolds(schedule, stills.join('\n'), samples.join('\n'));
	assert.equal(hold.holds, 3);
	assert.deepEqual(hold.stillAfterMs, [2300, 2500]);
	assert.deepEqual([hold.afterHold?.p50, hold.afterHold?.count], [90, 1]);
	assert.deepEqual([hold.others?.max, hold.others?.count], [30, 25]);
	const text = renderHoldSummary(hold);
	assert.match(text, /a still showed in 2, after 2300 to 2500 ms/);
	assert.match(text, /\| First change after a hold \| 90 \| 90 \| 90 \| 1 \|/);
});

test('a hold the capture shows as no pictures at all is a hold too', () => {
	const schedule = { origin: 0, moveMs: 2000, holdMs: 6000 };
	const picture = (shownMs, value, latencyMs) => JSON.stringify({ shownMs, codes: [{ value }, { value: value - latencyMs }], latencyMs });
	// Moving pictures 30 ms late, then nothing for 6 s, then the first change 70 ms late.
	const samples = [picture(0, 1000, 30), picture(33, 1033, 30), picture(66, 1066, 30), picture(6100, 7100, 70), picture(6133, 7133, 30)];
	const hold = summariseHolds(schedule, '', samples.join('\n'));
	assert.deepEqual([hold.afterHold?.p50, hold.afterHold?.count], [70, 1]);
	assert.deepEqual([hold.others?.max, hold.others?.count], [30, 4]);
});

test('a two-computer run names the other computer, its checkout and which one hosts', () => {
	assert.equal(latencyOptions('/root', ['--fps', '30']).frameRate, 30);
	assert.throws(() => latencyOptions('/root', ['--fps', '0']));
	assert.throws(() => latencyOptions('/root', ['--fps', '29.97']));
	const options = latencyOptions('/root', ['--remote', 'someone@studio.local', '--remote-root', '/home/someone/vnilink', '--hosts', 'remote', '--ssh-config', '/c']);
	assert.deepEqual(options.remote, { destination: 'someone@studio.local', root: '/home/someone/vnilink', hosts: 'remote', sshConfig: resolve('/c') });
	assert.equal(latencyOptions('/root', ['--remote', 'someone@studio.local', '--remote-root', '/r']).remote?.hosts, 'here');
	assert.throws(() => latencyOptions('/root', ['--remote', 'someone@studio.local']), /--remote-root/);
	assert.throws(() => latencyOptions('/root', ['--remote', 'someone@studio.local', '--remote-root', 'relative']), /--remote-root/);
	assert.throws(() => latencyOptions('/root', ['--remote', 'someone@studio.local', '--remote-root', '/r', '--hosts', 'both']));
	assert.throws(() => latencyOptions('/root', ['--remote', 'someone@studio.local', '--remote-root', '/r', '--hold', '5']), /--hold/);
	assert.throws(() => latencyOptions('/root', ['--hosts', 'remote']), /need --remote/);
});

test('a two-computer run is summarised from each side\'s own samples', async () => {
	const { renderAcrossSummary, summariseLatency, summarisePipeline, summariseViewerStatus } = await import('../../build/lib/latency.ts');
	const sample = fields => JSON.stringify({ at: 0, sample: fields });
	const host = summarisePipeline([
		sample({ codec: 'h265', width: 1920, height: 1080, capture_us: 2000, encode_us: 9000, encoder: 'nvenc', encoder_hardware: true, keyframes_sent: 3 }),
		sample({ codec: 'h265', width: 1920, height: 1080, capture_us: 4000, encode_us: 11000, encoder: 'nvenc', encoder_hardware: true, keyframes_sent: 4 }),
		sample({}),
	].join('\n'));
	assert.deepEqual([host.capture?.p50, host.capture?.p95, host.capture?.count], [2, 4, 2]);
	assert.deepEqual([host.encode?.p50, host.encode?.p95], [9, 11]);
	assert.deepEqual(host.last, {});
	const viewer = summarisePipeline([
		sample({ decoder_hardware: true, conditions: [{ condition: 'encoder.slow' }] }),
		sample({ decoder_hardware: true, conditions: [] }),
	].join('\n'));
	assert.deepEqual(viewer.conditions, ['encoder.slow']);
	const stages = [
		JSON.stringify({ at: 0, text: 'Decoding: 2.0 ms, 3.0 ms at most for 95%\n3 pictures replaced by newer ones before drawing', status: '58 fps · HEVC · 9.1 Mb/s · 4 ms' }),
		JSON.stringify({ at: 1, text: 'Decoding: 2.0 ms, 3.0 ms at most for 95%', status: '60 fps · HEVC · 9.4 Mb/s · 6 ms' }),
	].join('\n');
	const status = summariseViewerStatus(stages);
	assert.deepEqual([status.frameRate?.p50, status.frameRate?.p95, status.roundTrip?.p50, status.roundTrip?.p95, status.replaced?.count], [58, 60, 4, 6, 1]);
	const text = renderAcrossSummary(summarisePipeline(sample({ codec: 'h265', width: 1920, height: 1080, encoder: 'nvenc', encoder_hardware: true, capture_us: 2000, encode_us: 9000 })), viewer, summariseLatency('', stages), status);
	assert.match(text, /Codec h265, 1920x1080; host encoder nvenc \(hardware true/);
	assert.match(text, /\| Encode \(scaling and copies included\) \| 9\.0 \| 9\.0 \| 1 \|/);
	assert.match(text, /\| Pictures received a second \| 58\.0 \| 60\.0 \| 2 \|/);
	assert.match(text, /\| Round trip \(ms\) \| 4\.0 \| 6\.0 \| 2 \|/);
	assert.match(text, /\| Decoding \| 2\.0 \| 3\.0 \| 2 \|/);
	assert.match(text, /Conditions the viewer held: encoder\.slow/);
});
