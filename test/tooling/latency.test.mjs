import assert from 'node:assert/strict';
import test from 'node:test';
import { join, resolve } from 'node:path';
import { runInNewContext } from 'node:vm';
import { build } from 'esbuild';
import { latencyOptions } from '../../build/lib/latency.ts';

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
	assert.deepEqual(options, { seconds: 5, settleSeconds: 1, output: resolve('/out'), app: undefined });
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
