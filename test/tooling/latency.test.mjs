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
	assert.deepEqual(options, { seconds: 5, settleSeconds: 1, output: resolve('/out'), app: undefined, holdSeconds: 0 });
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
