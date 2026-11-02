// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'assert';
import { join, resolve } from 'node:path';
import { suite, test } from 'node:test';
import { runInNewContext } from 'node:vm';
import { CELLS, cells, stimulusPage } from '../../../test/latency/stimulus.ts';
import {
	agentLatencyEnvironment, latencyOptions, readerBuild, renderAcrossSummary, renderHoldSummary, renderLatencySummary, summariseHolds, summariseLatency,
	summarisePipeline, summariseViewerStatus
} from '../latency.ts';

/** The strip `crates/vnilink-media/src/tests/timecode.rs` expects from the reader's layout. */
const EXPECTED = '1010101001011100001111100101101000111100000111';

/** One JSON object per line, as the harness writes its logs. */
function jsonLines(entries: readonly unknown[]): string {
	return entries.map(entry => JSON.stringify(entry)).join('\n');
}

suite('latency', () => {
	suite('stimulus', () => {
		const strip = cells(0xA5C3E).map(dark => dark ? '1' : '0').join('');
		test('draws the strip the reader decodes', () => {
			assert.strictEqual(strip, EXPECTED);
		});
		test('draws every cell of a strip', () => {
			assert.strictEqual(strip.length, CELLS);
		});
		test('wraps values at 20 bits', () => {
			assert.deepStrictEqual(cells(1 << 20), cells(0));
		});
		const page = stimulusPage(30);
		const pageCells = runInNewContext(`${page.slice(page.indexOf('const VALUE_BITS'), page.indexOf('const canvas'))}; cells`) as (value: number) => ArrayLike<boolean>;
		for (const value of [0, 1, 0xA5C3E, 0xFFFFF, 123456789]) {
			test(`carries the module's strip for ${String(value)} in the page`, () => {
				// Copied out of the page's realm, whose arrays have their own prototype.
				assert.deepStrictEqual(Array.from(pageCells(value)), cells(value));
			});
		}
		test('sizes the page\'s cells as asked', () => {
			assert.match(page, /const cell = 30;/);
		});
		test('draws a page without changing detail by default', () => {
			assert.match(page, /const busy = false;/);
		});
		test('fills a busy page with changing detail', () => {
			assert.match(stimulusPage(30, undefined, true), /const busy = true;/);
		});
	});
	suite('options', () => {
		test('reads the sampling, settling and output options', () => {
			// Paths resolve the platform's way: `/out` is `C:\out` on Windows.
			assert.deepStrictEqual(latencyOptions('/root', ['--seconds', '5', '--settle', '1', '--output', '/out']), {
				seconds: 5, settleSeconds: 1, output: resolve('/out'), app: undefined, holdSeconds: 0, frameRate: undefined, bitrate: undefined, busy: false, peer: undefined
			});
		});
		test('accepts the largest bitrate', () => {
			assert.strictEqual(latencyOptions('/root', ['--fps', '60', '--bitrate', '50000']).bitrate, 50000);
		});
		test('accepts a busy stimulus across two computers', () => {
			assert.strictEqual(latencyOptions('/root', ['--busy', '--peer', 'agent.vnl', '--peer-check', '123123']).busy, true);
		});
		test('reads the hold', () => {
			assert.strictEqual(latencyOptions('/root', ['--hold', '6']).holdSeconds, 6);
		});
		test('reads an application bundle', () => {
			assert.strictEqual(latencyOptions('/root', ['--app', '/x/VniLink.app']).app, '/x/VniLink.app');
		});
		test('writes under .build/latency by default', () => {
			assert.ok(latencyOptions('/root', []).output.startsWith(resolve('/root', '.build', 'latency')));
		});
		test('resolves a relative output against the working directory', () => {
			assert.strictEqual(latencyOptions('/root', ['--output', 'relative/out']).output, join(process.cwd(), 'relative/out'));
		});
		const rejected: readonly (readonly [string, readonly string[]])[] = [
			['a busy stimulus on one computer', ['--busy']],
			['a zero bitrate', ['--bitrate', '0']],
			['a bitrate over 50000', ['--bitrate', '50001']],
			['a bitrate that is not a number', ['--bitrate', 'automatic']],
			['a hold over 60 seconds', ['--hold', '61']],
			['a fractional hold', ['--hold', '1.5']],
			['an application that is not a bundle', ['--app', '/x/VniLink']],
			['zero sampling seconds', ['--seconds', '0']],
			['fractional sampling seconds', ['--seconds', '2.5']],
			['a negative settling time', ['--settle', '-1']],
			['an unknown option', ['--unknown']],
		];
		for (const [description, args] of rejected) {
			test(`rejects ${description}`, () => {
				assert.throws(() => latencyOptions('/root', args));
			});
		}
	});
	suite('two computers', () => {
		test('reads the frame rate', () => {
			assert.strictEqual(latencyOptions('/root', ['--fps', '30']).frameRate, 30);
		});
		test('names the agent by its file and digits, and which computer hosts', () => {
			assert.deepStrictEqual(latencyOptions('/root', ['--peer', 'agent.vnl', '--peer-check', '042123', '--hosts', 'peer']).peer, { file: resolve('agent.vnl'), check: '042123', hosts: 'peer' });
		});
		test('hosts on this computer by default', () => {
			assert.strictEqual(latencyOptions('/root', ['--peer', '/a.vnl', '--peer-check', '123123']).peer?.hosts, 'here');
		});
		const rejected: readonly (readonly [string, readonly string[], RegExp | ErrorConstructor])[] = [
			['a zero frame rate', ['--fps', '0'], Error],
			['a fractional frame rate', ['--fps', '29.97'], Error],
			['an agent without its digits', ['--peer', '/a.vnl'], /go together/],
			['digits without an agent', ['--peer-check', '123123'], /go together/],
			['digits that are not six', ['--peer', '/a.vnl', '--peer-check', '12'], /six digits/],
			['both computers hosting', ['--peer', '/a.vnl', '--peer-check', '123123', '--hosts', 'both'], Error],
			['a hold across two computers', ['--peer', '/a.vnl', '--peer-check', '123123', '--hold', '5'], /--hold/],
			['the peer hosting without an agent', ['--hosts', 'peer'], /needs --peer/],
			// SSH is gone: its options are unknown.
			['the removed SSH remote', ['--remote', 'someone@studio.local'], Error],
			['the removed SSH remote root', ['--remote-root', '/r'], Error],
		];
		for (const [description, args, message] of rejected) {
			test(`rejects ${description}`, () => {
				assert.throws(() => latencyOptions('/root', args), message);
			});
		}
		const set = { VNILINK_LATENCY_DIR: '/a/rendezvous', VNILINK_LATENCY_OUTPUT: '/a/output', VNILINK_LATENCY_SCREEN_GRANT: '/c/.build/latency/host-screen-grant', VNILINK_TEST_PROFILE: '/a/profile' };
		test('runs a side on the agent with the agent\'s folders, always across, and only whitelisted numbers in range', () => {
			assert.deepStrictEqual(agentLatencyEnvironment('viewer', {
				...set, VNILINK_LATENCY_FPS: '60', VNILINK_LATENCY_ACROSS: '0', VNILINK_LATENCY_BITRATE: '60; rm', VNILINK_LATENCY_CONSENT_MS: '600001', PATH: '/usr/bin', VNILINK_LATENCY_ROLE: 'host',
			}), { VNILINK_LATENCY_ROLE: 'viewer', VNILINK_LATENCY_ACROSS: '1', ...set, VNILINK_LATENCY_FPS: '60' });
		});
		test('refuses a side on the agent without its output folder', () => {
			assert.throws(() => agentLatencyEnvironment('host', { ...set, VNILINK_LATENCY_OUTPUT: '' }), /VNILINK_LATENCY_OUTPUT/);
		});
		const sample = (fields: Readonly<Record<string, unknown>>) => ({ at: 0, sample: fields });
		const host = summarisePipeline(jsonLines([
			sample({ codec: 'h265', width: 1920, height: 1080, capture_us: 2000, encode_us: 9000, encoder: 'nvenc', encoder_hardware: true, keyframes_sent: 3 }),
			sample({ codec: 'h265', width: 1920, height: 1080, capture_us: 4000, encode_us: 11000, encoder: 'nvenc', encoder_hardware: true, keyframes_sent: 4 }),
			sample({}),
		]));
		const viewer = summarisePipeline(jsonLines([
			sample({ decoder_hardware: true, conditions: [{ condition: 'encoder.slow' }] }),
			sample({ decoder_hardware: true, conditions: [] }),
		]));
		const stages = jsonLines([
			{ at: 0, text: 'Decoding: 2.0 ms, 3.0 ms at most for 95%\n3 pictures replaced by newer ones before drawing', status: '58 fps · HEVC · 9.1 Mb/s · 4 ms' },
			{ at: 1, text: 'Decoding: 2.0 ms, 3.0 ms at most for 95%', status: '60 fps · HEVC · 9.4 Mb/s · 6 ms' },
		]);
		const status = summariseViewerStatus(stages);
		test('takes the host\'s capture times from its samples', () => {
			assert.deepStrictEqual([host.capture?.p50, host.capture?.p95, host.capture?.count], [2, 4, 2]);
		});
		test('takes the host\'s encode times from its samples', () => {
			assert.deepStrictEqual([host.encode?.p50, host.encode?.p95], [9, 11]);
		});
		test('keeps a side\'s last sample', () => {
			assert.deepStrictEqual(host.last, {});
		});
		test('collects every condition the viewer held', () => {
			assert.deepStrictEqual(viewer.conditions, ['encoder.slow']);
		});
		test('reads the viewer status beside its codec and bitrate', () => {
			const { frameRate, roundTrip, replaced } = status;
			assert.deepStrictEqual([frameRate?.p50, frameRate?.p95, roundTrip?.p50, roundTrip?.p95, replaced?.count], [58, 60, 4, 6, 1]);
		});
		const last = summarisePipeline(jsonLines([sample({ codec: 'h265', width: 1920, height: 1080, encoder: 'nvenc', encoder_hardware: true, capture_us: 2000, encode_us: 9000 })]));
		const text = renderAcrossSummary(last, viewer, summariseLatency('', stages), status);
		const rows: readonly (readonly [string, RegExp])[] = [
			['the codec, size and host encoder', /Codec h265, 1920x1080; host encoder nvenc \(hardware true/],
			['the host\'s encode time', /\| Encode \(scaling and copies included\) \| 9\.0 \| 9\.0 \| 1 \|/],
			['the pictures the viewer received a second', /\| Pictures received a second \| 58\.0 \| 60\.0 \| 2 \|/],
			['the viewer\'s round trip', /\| Round trip \(ms\) \| 4\.0 \| 6\.0 \| 2 \|/],
			['the stages the viewer measures', /\| Decoding \| 2\.0 \| 3\.0 \| 2 \|/],
			['the conditions the viewer held', /Conditions the viewer held: encoder\.slow/],
		];
		for (const [description, pattern] of rows) {
			test(`summarises ${description} across two computers`, () => {
				assert.match(text, pattern);
			});
		}
	});
	suite('summary', () => {
		const status = summariseViewerStatus(jsonLines([
			{ at: 1, text: '', status: '60 fps · 4 ms · Direct' },
			{ at: 2, text: '3 pictures replaced by newer ones before drawing', status: '58 fps · 6 ms · Relay' },
		]));
		test('reads the viewer\'s frame rate from its status', () => {
			assert.deepStrictEqual(status.frameRate, { p50: 58, p95: 60, count: 2 });
		});
		test('reads the viewer\'s round trip from its status', () => {
			assert.deepStrictEqual(status.roundTrip, { p50: 4, p95: 6, count: 2 });
		});
		test('reads the pictures the viewer replaced from its stages', () => {
			assert.deepStrictEqual(status.replaced, { p50: 3, p95: 3, count: 1 });
		});
		const summary = summariseLatency(jsonLines([10, 30, 20, null, 40].map(latencyMs => ({ shownMs: 1, codes: [], latencyMs }))), jsonLines([
			{ at: 1, text: 'Network: about 0 ms each way\nDecoding: 2.0 ms, 5.0 ms at most for 95%\nDrawing: 0.5 ms, 1.0 ms at most for 95%' },
			{ at: 2, text: 'Decoding: 4.0 ms, 9.0 ms at most for 95%' },
			{ at: 3, text: 'Decoding: 3.0 ms, 6.0 ms at most for 95%' },
		]));
		test('counts every picture the reader read', () => {
			assert.strictEqual(summary.pictures, 5);
		});
		test('takes glass to glass over the pictures with a reading', () => {
			assert.deepStrictEqual(summary.total, { p50: 20, p95: 40, p99: 40, max: 40, count: 4 });
		});
		test('takes the median of a stage over its windows', () => {
			assert.deepStrictEqual(summary.stages.get('Decoding'), { p50: 3, p95: 6, windows: 3 });
		});
		test('keeps a stage seen in one window', () => {
			assert.deepStrictEqual(summary.stages.get('Drawing'), { p50: 0.5, p95: 1, windows: 1 });
		});
		test('leaves out the network estimate', () => {
			assert.strictEqual(summary.stages.has('Network'), false);
		});
		test('renders glass to glass as a table row', () => {
			assert.match(renderLatencySummary(summary), /\| Glass to glass \| 20 \| 40 \| 40 \| 40 \|/);
		});
		test('has no glass to glass without pictures', () => {
			assert.strictEqual(summariseLatency('', '').total, undefined);
		});
		const empty = summariseLatency('', '');
		const cpu = renderLatencySummary(empty, { host: { Browser: 0.1, 'VniLink Engine': 1.25 }, viewer: { GPU: 0.5 } });
		test('shows the host\'s CPU, largest process first', () => {
			assert.match(cpu, /\| host \| 1\.35 \| VniLink Engine 1\.25, Browser 0\.10 \|/);
		});
		test('shows the viewer\'s CPU', () => {
			assert.match(cpu, /\| viewer \| 0\.50 \| GPU 0\.50 \|/);
		});
		test('leaves CPU out when it was not measured', () => {
			assert.doesNotMatch(renderLatencySummary(empty), /CPU while/);
		});
		test('names the encoder the host had open and what it read', () => {
			assert.match(renderLatencySummary(empty, undefined, { encoder: 'mf-intel', hardware: true, input: 'gpu-texture' }), /Host encoder: mf-intel, hardware, input gpu-texture\./);
		});
		test('says which encoder details are unknown', () => {
			assert.match(renderLatencySummary(empty, undefined, {}), /Host encoder: unknown, kind unknown, input unknown\./);
		});
		test('leaves the encoder out when it was not recorded', () => {
			assert.doesNotMatch(renderLatencySummary(empty), /Host encoder/);
		});
	});
	suite('hold mode', () => {
		const schedule = { origin: 0, moveMs: 2000, holdMs: 6000 };
		// The viewer reads its count every 100 ms; a still shows 2.3 s into the
		// first hold and 2.5 s into the second; the third brings none.
		const stills: { at: number; shown: number }[] = [];
		for (let at = 1000; at <= 25000; at += 100) {
			stills.push({ at, shown: (at >= 4300 ? 1 : 0) + (at >= 12500 ? 1 : 0) });
		}
		// The reader: moving pictures 30 ms late, a 6 s hold without a new
		// strip, then the first change 90 ms late, then 30 ms again.
		// Windows-like: pictures keep coming while the stimulus holds, a few a second.
		const samples: { shownMs: number; codes: { value: number }[]; latencyMs: number | null }[] = [];
		let value = 100;
		let shown = 0;
		for (let index = 0; index < 20; index++) {
			samples.push({ shownMs: shown += 33, codes: [{ value: value += 16 }, { value: value - 30 }], latencyMs: 30 });
		}
		for (let index = 0; index < 40; index++) {
			samples.push({ shownMs: shown += 150, codes: [{ value }, { value }], latencyMs: null });
		}
		samples.push({ shownMs: shown += 33, codes: [{ value: value += 6000 }, { value: value - 90 }], latencyMs: 90 });
		for (let index = 0; index < 5; index++) {
			samples.push({ shownMs: shown += 33, codes: [{ value: value += 16 }, { value: value - 30 }], latencyMs: 30 });
		}
		const hold = summariseHolds(schedule, jsonLines(stills), jsonLines(samples));
		test('counts every hold', () => {
			assert.strictEqual(hold.holds, 3);
		});
		test('says when each still showed', () => {
			assert.deepStrictEqual(hold.stillAfterMs, [2300, 2500]);
		});
		test('measures the first change after a hold apart', () => {
			assert.deepStrictEqual([hold.afterHold?.p50, hold.afterHold?.count], [90, 1]);
		});
		test('measures every other picture together', () => {
			assert.deepStrictEqual([hold.others?.max, hold.others?.count], [30, 25]);
		});
		test('says when stills showed in the summary', () => {
			assert.match(renderHoldSummary(hold), /a still showed in 2, after 2300 to 2500 ms/);
		});
		test('renders the first change after a hold as a table row', () => {
			assert.match(renderHoldSummary(hold), /\| First change after a hold \| 90 \| 90 \| 90 \| 1 \|/);
		});

		// Moving pictures 30 ms late, then nothing for 6 s, then the first change 70 ms late.
		const picture = (shownMs: number, code: number, latencyMs: number) => ({ shownMs, codes: [{ value: code }, { value: code - latencyMs }], latencyMs });
		const silent = summariseHolds(schedule, '', jsonLines([picture(0, 1000, 30), picture(33, 1033, 30), picture(66, 1066, 30), picture(6100, 7100, 70), picture(6133, 7133, 30)]));
		test('finds the first change after a hold the capture shows as no pictures', () => {
			assert.deepStrictEqual([silent.afterHold?.p50, silent.afterHold?.count], [70, 1]);
		});
		test('measures the other pictures around a hold the capture shows as no pictures', () => {
			assert.deepStrictEqual([silent.others?.max, silent.others?.count], [30, 4]);
		});
	});
	suite('reader build', () => {
		const windows = readerBuild('vnilink-windows', 'x86_64-pc-windows-gnu', { PATH: 'C:\\mingw\\bin' }, () => true);
		test('shares the addon\'s build of the codecs for a target', () => {
			assert.deepStrictEqual(windows.args, ['build', '--locked', '--target-dir', 'target/electron', '--target', 'x86_64-pc-windows-gnu', '--release', '-p', 'vnilink-windows', '--example', 'glass_latency']);
		});
		test('links with the MinGW tools the addon build uses', () => {
			assert.strictEqual(windows.env.CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER, 'gcc');
		});
		const mac = readerBuild('vnilink-macos', undefined, { PATH: '/bin' });
		test('builds for the host\'s default target when none is named', () => {
			assert.deepStrictEqual(mac.args, ['build', '--locked', '--target-dir', 'target/electron', '--release', '-p', 'vnilink-macos', '--example', 'glass_latency']);
		});
		test('keeps the environment as it is for the host\'s default target', () => {
			assert.deepStrictEqual(mac.env, { PATH: '/bin' });
		});
	});
});
