// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

/**
 * Glass-to-glass latency (`npm run measure:latency`): how long a change on
 * a computer's screen takes to appear in a VniLink viewer. It runs on
 * macOS, Windows and Linux, on one computer: host, viewer and reader all run
 * on it. Use it to record numbers before and after a capture, encode,
 * transport, decode or drawing change. A second computer (`--peer`) is
 * optional and never required.
 *
 * # How it works
 *
 * Two temporary VniLink instances, each with its own identity and profile,
 * pair and stream over loopback:
 * - the host hosts the screen from a temporary engine that may capture, and
 *   opens a stimulus window across the top of the main display; on every
 *   animation frame the stimulus draws a strip of black and white cells
 *   carrying that frame's time in ms (`test/latency/stimulus.ts`, whose
 *   layout is `vnilink_media::measure::timecode`);
 * - the viewer shows the host's screen in a real workbench window below the
 *   strip, so its picture contains a copy of the strip, older by the
 *   latency;
 * - the reader captures the display and decodes every strip in each picture
 *   (`vnilink_media::measure::glass`). It is the `glass_latency` example of
 *   `vnilink-macos` (ScreenCaptureKit, timing a picture by its
 *   `displayTime`), `vnilink-windows` (Desktop Duplication, `LastPresentTime`)
 *   or `vnilink-linux` (the ScreenCast portal, when PipeWire handed the
 *   picture over). The live strip and its copy come from the same
 *   composited picture, so their difference is the latency, with no clocks
 *   to align; the display's scan-out and the reader's own capture delay are
 *   shared by both strips and cancel out.
 *
 * The viewer shows the display it is on, so its picture holds smaller
 * copies of itself. Those are older still, and the reader ignores them.
 *
 * # What it proves and what it does not
 *
 * It proves the time from a picture on this screen to the same picture shown
 * by a viewer through the product's path: the platform's capture and encoder
 * on the host (ScreenCaptureKit and VideoToolbox; Desktop Duplication and
 * Media Foundation; the RemoteDesktop portal, PipeWire and VA-API or a
 * software encoder), QUIC on loopback, the utility process, WebCodecs and
 * the canvas in the viewer, and the compositor.
 *
 * It does not cover a network (loopback has none; the impaired-network
 * harness adds that), panel response (both strips share it), another
 * computer or operating system (each run measures the one it ran on), or
 * independent hardware: host and viewer share one CPU, GPU and media engine,
 * which inflates encode and decode times compared with two computers. The
 * result is quantised to the display's refresh period (16.7 ms at 60 Hz), so
 * read percentiles over many samples, not single readings.
 *
 * # Running it
 *
 * At the computer, in its signed-in desktop or graphical session, after
 * `npm run build`. `npm run doctor` says what it can check beforehand.
 * - macOS: both sides start through LaunchServices (`open`), so macOS asks
 *   Screen Recording of the development VniLink (`.build/electron`) and not
 *   of the terminal that started the task. On the first run the host asks and
 *   stops; allow it in System Settings > Privacy & Security > Screen & System
 *   Audio Recording, then run again. macOS grants an ad-hoc signed
 *   application by its code hash, so each copy needs its own grant; `--app`
 *   names an application that already has one. It only hosts Electron: the
 *   code it runs is this checkout's `out/`. The host's engine prepares video
 *   only, so the viewer cannot control the host. The viewer's engine also
 *   prepares native video, without which it refuses the screen; on the same
 *   computer that slows the host's capture reads (about 19 ms at p50), a
 *   cost of measuring both sides on one computer, included in results since
 *   2026-11-01. Keep the display awake and uncovered.
 * - Windows: Desktop Duplication captures only the signed-in user's desktop
 *   and fails in a session that has none, such as a service or a remote
 *   shell, so run it from a terminal on that desktop. Windows asks no
 *   permission.
 * - Linux: the sides and the reader need the graphical session's bus and
 *   the desktop portal. The portal asks someone at the computer twice on
 *   the first run, once for the host and once for the reader (choose the
 *   display both times; each waits up to five minutes). The answers are kept
 *   as restore tokens in `.build/latency` (`host-screen-grant`,
 *   `reader-screen-grant`, readable by this user only) and later runs share
 *   the display without asking; delete them to be asked again. The sides run
 *   under XWayland, because a Wayland window cannot place itself. Silence
 *   notifications first: a banner across the top of the display covers the
 *   strip and no picture can be read. The reader times a picture by when
 *   PipeWire delivered it, which both strips share, so the latency is
 *   unaffected but hold mode's showing times are a little late.
 *
 * # Hold mode
 *
 * `--hold <seconds>` measures still pictures: when one shows after the
 * screen stops changing, and whether the first change after it is held up.
 * Nothing on the captured display may change during a hold, so the
 * stimulus fills the display with its strip above a panel of coloured text
 * (what a still is for: the host sends one only when it beats 4:2:0) and
 * covers the menu bar and Dock; the viewer covers the lower half and is
 * hidden while the stimulus holds, still receiving and decoding; its status
 * bar, which changes every second, is hidden; and it runs at the Smooth
 * preset, whose budget a 1080p still of this panel fits. Another
 * application's window above everything keeps the screen from holding. It
 * is measured on macOS and Linux; on Windows it runs the same but is
 * unmeasured.
 *
 * # Two computers
 *
 * `--peer <file.vnl> --peer-check <digits>` runs one side here and the other
 * on a second computer, so each side has its own CPU, GPU and media engine
 * and the stream crosses a real network. No screen shows both strips, so
 * there is no glass-to-glass total and no reader. Each side reports itself
 * once a second instead: the host's capture and encode times (scaling and
 * copies are inside the encode time), encoder, keyframes, reopens and drops
 * (`host-stages.jsonl`), and the viewer's stage times and status line
 * (`stages.jsonl`) and pipeline (`viewer-pipeline.jsonl`). The network leg
 * is the round trip, since the clocks differ. Both viewers prepare native
 * video, as the product's do.
 *
 * The other computer runs the test agent, built from the same commit
 * (`node build/run.ts testkit:agent --allow screen`; the sides show and read
 * its real screen, so the agent refuses them without `--allow screen`). The
 * agent prints digits; the run shows six of its own; the person at the
 * agent types them on its terminal, and nothing is approved without that.
 * The agent starts its side from its own checkout (`testkit:launch`, the
 * launch a run on one computer uses), in folders of its own that it removes
 * when the run stops, and only when `out/latency.cjs` there is exactly what
 * its sources build. A run names only the side and its numbers
 * (`agentLatencyVariables`); the agent refuses any other variable and is
 * never given a path or a program. The two sides' rendezvous files travel as
 * messages over the paired connection (`agentSideTakes`), each checked
 * again by the agent. A host on the agent's computer shows its screen only
 * to the viewer the run names, and only to be seen. Stopping the run, the
 * agent losing it, or Ctrl-C on the agent ends the side: the agent writes
 * `done`, gives it 30 s to quit, then ends its process group (on Windows,
 * the job object it runs in). A macOS or Windows agent starts a side the
 * same way as a Linux one, but only a Linux agent has been exercised across
 * two computers.
 *
 * # Evidence
 *
 * `.build/latency/<time>/` (or `--output`) holds `summary.md` (glass-to-
 * glass percentiles and the viewer's stage times), `samples.jsonl` (one
 * line per captured picture), `stages.jsonl`, `host-start.log` and
 * `viewer-start.log` (what each side saw while the stream started),
 * `host.log`, `viewer.log`, `host-pipeline.json` (the encoder, whether it is
 * hardware, what it read pictures from, keyframes, reopens and drops) and
 * `run.json` (the options and machine); hold mode adds `schedule.json` and
 * `stills.jsonl`. The reader prints the p50, p95, p99 and maximum when it
 * finishes.
 *
 * # Options
 *
 * `latencyOptions.ts` lists them, with their defaults.
 */
import { mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { assertNativeFresh } from '../native.ts';
import { osName } from '../platform.ts';
import { ProcessRunner } from '../processes.ts';
import { openAgentSession, type AgentSession } from '../remote.ts';
import type { agentLatencyVariables } from './latencyAgent.ts';
import { invitationKey } from './latencyAgent.ts';
import { latencyOptions, type LatencyOptions, type LatencyPeer } from './latencyOptions.ts';
import { arrival, latencyPlatform, publish, readerBuild, relayRendezvous, settledWithin } from './latencySides.ts';
import { latencyEncoder, renderAcrossSummary, renderHoldSummary, renderLatencySummary, summariseHolds, summariseLatency, summarisePipeline, summariseViewerStatus, type HoldSchedule } from './latencySummary.ts';

/**
 * One side on this computer and the other on the test agent's computer,
 * the pair a person uses: each side's own stage times, the host's capture
 * and encode and the viewer's queue, decode and drawing, with the round
 * trip for the network. No screen shows both, so there is no
 * glass-to-glass total.
 */
async function measureAcross(root: string, signal: AbortSignal, options: LatencyOptions, peer: LatencyPeer): Promise<void> {
	await assertNativeFresh(root);
	const processes = new ProcessRunner(signal);
	const work = await mkdtemp(join(tmpdir(), 'vl-'));
	const here: 'host' | 'viewer' = peer.hosts === 'here' ? 'host' : 'viewer';
	const there: 'host' | 'viewer' = here === 'host' ? 'viewer' : 'host';
	const rendezvous = join(work, 'rendezvous');
	let session: AgentSession | undefined;
	let started = false;
	// Changed by the sides and the relay while this function waits.
	const state = { stopped: false, localExited: false, peerRunning: true };
	const sides: Promise<unknown>[] = [];
	let fetched = false;
	/** The agent's side's evidence joins this side's, once, for a run that failed too. */
	const fetchEvidence = async (): Promise<void> => {
		if (fetched || !session || !started) {
			return;
		}
		fetched = true;
		await session.fetch(options.output);
	};
	try {
		await mkdir(options.output, { recursive: true });
		await mkdir(rendezvous);
		session = await openAgentSession(root, peer, signal);
		const grants = join(root, '.build/latency');
		await mkdir(grants, { recursive: true, mode: 0o700 });
		// A Linux host's portal asks someone there; the other side waits as long.
		const hostOs = here === 'host' ? osName : session.platform.os;
		const shared: Record<keyof typeof agentLatencyVariables, string> = {
			VNILINK_LATENCY_FPS: String(options.frameRate ?? 0),
			VNILINK_LATENCY_BITRATE: String(options.bitrate ?? 0),
			VNILINK_LATENCY_BUSY: options.busy ? '1' : '0',
			VNILINK_LATENCY_CONSENT_MS: String(hostOs === 'linux' ? 5 * 60000 : 0),
		};
		const platform = await latencyPlatform(root, options.app, processes, signal);
		const profile = join(work, here);
		await mkdir(profile);
		const local = await platform.launch(here, {
			...shared,
			// Each side on its own computer, as a person uses them; the agent
			// sets it for its side itself.
			VNILINK_LATENCY_ACROSS: '1',
			VNILINK_LATENCY_ROLE: here,
			VNILINK_LATENCY_DIR: rendezvous,
			VNILINK_LATENCY_OUTPUT: options.output,
			VNILINK_LATENCY_SCREEN_GRANT: join(grants, 'host-screen-grant'),
			VNILINK_TEST_PROFILE: profile,
		}, join(options.output, `${here}.log`));
		sides.push(local.result.catch(() => undefined).finally(() => {
			state.localExited = true;
		}));
		// The agent starts the other side from its own checkout, with its own
		// folders and grant; this side names only the scenario and the numbers,
		// and, for a host there, the one viewer it may show its screen to.
		const params: Record<string, unknown> = { env: shared };
		if (there === 'host') {
			await arrival(rendezvous, 'viewer', 120000, () => state.localExited, signal);
			const viewerKey = invitationKey((JSON.parse(await readFile(join(rendezvous, 'viewer.json'), 'utf8')) as Record<string, unknown>).token);
			if (!viewerKey) {
				throw new Error('The viewer here published no invitation');
			}
			params.viewer_key = viewerKey;
		}
		await session.start(`latency-${there}`, params);
		started = true;
		const relaying = relayRendezvous(session, rendezvous, there, () => state.stopped, running => {
			state.peerRunning = running;
		}, signal).catch((error: unknown) => {
			state.peerRunning = false;
			throw error;
		});
		relaying.catch(() => undefined);
		const exited = (): boolean => state.localExited || !state.peerRunning;
		// The portal on a Linux host asks someone there once; its answer is kept.
		await arrival(rendezvous, 'viewer-streaming', 120000 + 5 * 60000, exited, signal);
		await new Promise(resolve => setTimeout(resolve, options.settleSeconds * 1000));
		await publish(rendezvous, 'read', { seconds: options.seconds });
		await arrival(rendezvous, 'read-done', (options.seconds + 60) * 1000, exited, signal);
		await arrival(rendezvous, 'viewer-cpu', 5000, () => false, signal).catch(() => undefined);
		await publish(rendezvous, 'done', {});
		// Both sides quit on `done`, the agent's once the relay carries it.
		const quit = Date.now() + 30000;
		while ((!state.localExited || state.peerRunning) && Date.now() < quit) {
			await new Promise(resolve => setTimeout(resolve, 250));
		}
		state.stopped = true;
		await relaying.catch(() => undefined);
		await fetchEvidence();
		const read = async (name: string): Promise<string> => readFile(join(options.output, name), 'utf8').catch(() => '');
		const cpuOf = async (name: string): Promise<Record<string, number>> => {
			const value: unknown = JSON.parse(await readFile(join(rendezvous, `${name}.json`), 'utf8').catch(() => '{}'));
			return typeof value === 'object' && value !== null ? Object.fromEntries(Object.entries(value).filter((entry): entry is [string, number] => typeof entry[1] === 'number')) : {};
		};
		const stages = await read('stages.jsonl');
		const summary = renderAcrossSummary(
			summarisePipeline(await read('host-stages.jsonl')),
			summarisePipeline(await read('viewer-pipeline.jsonl')),
			summariseLatency('', stages),
			summariseViewerStatus(stages),
			{ host: await cpuOf('host-cpu'), viewer: await cpuOf('viewer-cpu') },
		);
		// Each computer by what it runs, never by its name.
		const agent = `${session.platform.os} ${session.platform.arch} (the agent)`;
		const run = {
			seconds: options.seconds, settle: options.settleSeconds, fps: options.frameRate ?? null, bitrate: options.bitrate ?? null, busy: options.busy,
			host: here === 'host' ? `${osName} ${process.arch} (here)` : agent,
			viewer: here === 'viewer' ? `${osName} ${process.arch} (here)` : agent,
			date: new Date().toISOString(),
		};
		await writeFile(join(options.output, 'run.json'), JSON.stringify(run, null, 2));
		await writeFile(join(options.output, 'summary.md'), summary);
		console.log(summary);
		console.log(`Evidence: ${options.output}`);
	} finally {
		state.stopped = true;
		await publish(rendezvous, 'done', {}).catch(() => undefined);
		await settledWithin(Promise.all(sides), 20000);
		await fetchEvidence().catch(() => undefined);
		// The agent tells its side `done`, ends it if it does not quit, and
		// removes its folders; its checkout keeps only the portal's grant.
		if (started) {
			await session?.stop().catch(() => undefined);
		}
		await session?.close();
		await processes.stop();
		await rm(work, { recursive: true, force: true });
	}
}

export async function measureLatency(root: string, signal: AbortSignal, args: readonly string[]): Promise<void> {
	const options = latencyOptions(root, args);
	if (options.peer) {
		await measureAcross(root, signal, options, options.peer);
		return;
	}
	await assertNativeFresh(root);
	const processes = new ProcessRunner(signal);
	const work = await mkdtemp(join(tmpdir(), 'vnilink-latency-'));
	const sides: Promise<unknown>[] = [];
	try {
		await mkdir(options.output, { recursive: true });
		const platform = await latencyPlatform(root, options.app, processes, signal);
		// The reader is built before the sides start, so its compile does not
		// count against their timeouts.
		const build = readerBuild(platform.readerCrate, platform.readerTarget);
		await processes.run('cargo', build.args, { cwd: root, env: build.env });
		const rendezvous = join(work, 'rendezvous');
		await mkdir(rendezvous);
		// Where the desktop portal's grants are kept between runs, for the
		// host's screen and for the reader's, so each is asked for once.
		const grants = join(root, '.build/latency');
		await mkdir(grants, { recursive: true, mode: 0o700 });
		let exited = false;
		for (const role of ['host', 'viewer'] as const) {
			const profile = join(work, role);
			await mkdir(profile);
			const environment = {
				VNILINK_TEST_PROFILE: profile,
				VNILINK_LATENCY_ROLE: role,
				VNILINK_LATENCY_DIR: rendezvous,
				VNILINK_LATENCY_OUTPUT: options.output,
				VNILINK_LATENCY_HOLD_MS: String(options.holdSeconds * 1000),
				VNILINK_LATENCY_FPS: String(options.frameRate ?? 0),
				VNILINK_LATENCY_BITRATE: String(options.bitrate ?? 0),
				VNILINK_LATENCY_BUSY: options.busy ? '1' : '0',
				VNILINK_LATENCY_SCREEN_GRANT: join(grants, 'host-screen-grant'),
			};
			const side = await platform.launch(role, environment, join(options.output, `${role}.log`));
			sides.push(side.result.catch(() => undefined).finally(() => {
				exited = true;
			}));
		}
		await arrival(rendezvous, 'viewer-streaming', 120000 + platform.consentMs, () => exited, signal).catch(async (error: unknown) => {
			// Each side says why in its log.
			const logs = await Promise.all(['host', 'viewer'].map(async role => `${role}:\n${(await readFile(join(options.output, `${role}.log`), 'utf8').catch(() => '')).slice(-1500)}`));
			throw new Error(`${error instanceof Error ? error.message : String(error)}\n${logs.join('\n')}`);
		});
		await new Promise(resolve => setTimeout(resolve, options.settleSeconds * 1000));
		const samples = join(options.output, 'samples.jsonl');
		// The host runs the reader, under its own Screen Recording permission.
		await publish(rendezvous, 'read', { reader: platform.reader, seconds: options.seconds, samples, grant: join(grants, 'reader-screen-grant') });
		await arrival(rendezvous, 'read-done', (options.seconds + 60) * 1000 + platform.consentMs, () => exited, signal);
		const read: unknown = JSON.parse(await readFile(join(rendezvous, 'read-done.json'), 'utf8'));
		if (typeof read !== 'object' || read === null || !('code' in read) || read.code !== 0) {
			throw new Error(`The reader failed: ${JSON.stringify(read)}`);
		}
		const run = {
			seconds: options.seconds, settle: options.settleSeconds, hold: options.holdSeconds,
			platform: process.platform, arch: process.arch, date: new Date().toISOString(),
		};
		await writeFile(join(options.output, 'run.json'), JSON.stringify(run, null, 2));
		// The viewer samples once a second: let it report before it is told to quit.
		await arrival(rendezvous, 'viewer-cpu', 5000, () => false, signal).catch(() => undefined);
		await publish(rendezvous, 'done', {});
		const stages = await readFile(join(options.output, 'stages.jsonl'), 'utf8').catch(() => '');
		const cpuOf = async (name: string): Promise<Record<string, number>> => {
			const value: unknown = JSON.parse(await readFile(join(rendezvous, `${name}.json`), 'utf8').catch(() => '{}'));
			return typeof value === 'object' && value !== null ? Object.fromEntries(Object.entries(value).filter((entry): entry is [string, number] => typeof entry[1] === 'number')) : {};
		};
		const cpu = { host: await cpuOf('host-cpu'), viewer: await cpuOf('viewer-cpu') };
		await writeFile(join(options.output, 'cpu.json'), JSON.stringify(cpu, null, 2));
		const pipeline: unknown = JSON.parse(await readFile(join(rendezvous, 'host-pipeline.json'), 'utf8').catch(() => '{}'));
		await writeFile(join(options.output, 'host-pipeline.json'), JSON.stringify(pipeline, null, 2));
		const encoder = latencyEncoder(pipeline);
		let summary = renderLatencySummary(summariseLatency(await readFile(samples, 'utf8'), stages), cpu, encoder);
		if (options.holdSeconds > 0) {
			const schedule = JSON.parse(await readFile(join(options.output, 'schedule.json'), 'utf8')) as HoldSchedule;
			const stills = await readFile(join(options.output, 'stills.jsonl'), 'utf8').catch(() => '');
			summary += `\n${renderHoldSummary(summariseHolds(schedule, stills, await readFile(samples, 'utf8')))}`;
		}
		await writeFile(join(options.output, 'summary.md'), summary);
		console.log(summary);
		console.log(`Evidence: ${options.output}`);
	} finally {
		// The sides quit on `done`; stopping `open` would not stop them.
		await publish(join(work, 'rendezvous'), 'done', {}).catch(() => undefined);
		// A side's process (`open -W` on macOS) returns when it has quit; its
		// profile is then free to remove.
		await settledWithin(Promise.all(sides), 20000);
		await processes.stop();
		await rm(work, { recursive: true, force: true });
	}
}
