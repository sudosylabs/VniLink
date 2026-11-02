// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

/**
 * Impaired-network measurement (`npm run measure:impaired`): file transfer
 * and desktop video between two VniLink engines under controlled loss, delay
 * and rate limits. Use it to record numbers before and after a transfer or
 * video change.
 *
 * # What it proves and what it does not
 *
 * Two engines run in a Linux network namespace pair joined by a `tc netem`
 * link (`netem.ts`). The results show how the Rust engines behave over an
 * impaired link on Linux: the transport, the job engine, the datagram video
 * path, keyframe requests and adaptation.
 *
 * It does not show macOS or Windows behaviour. The video source and encoder
 * are synthetic, so it says nothing about ScreenCaptureKit, VideoToolbox,
 * Media Foundation, the renderer or WebCodecs. A container on macOS or
 * Windows runs in a virtual machine, so absolute throughput reflects that
 * machine and the CPU contention between the two engines. Compare runs made
 * on the same machine, never with a number from elsewhere.
 *
 * # Running it
 *
 * `measure:impaired` builds the project image (`build/containers/Dockerfile.linux`),
 * mounts the checkout into a privileged container and runs
 * `measure:impaired:linux` there; on a Linux host that task runs directly as
 * root. Arguments pass through, for example `--profiles quick`. Evidence
 * lands in `.build/impaired-network/<time>/` (`--output` names another
 * directory for the inner task):
 * - `summary.md`: one table per scenario;
 * - `results.json`: options, profiles, every scenario result, and veth and
 *   qdisc counters for each run;
 * - `environment.txt`: kernel, toolchain, `tc` version, CPU count, memory;
 * - `<profile>/<scenario>.json`: the raw scenario output.
 *
 * The release test build is cached in a volume per checkout, and so are the
 * Node dependencies the task runner loads, so two checkouts never run each
 * other's binaries. A run refuses to start while another is in progress: two
 * runs on one machine compete for its CPUs. The image builds with one cargo
 * job unless `CARGO_BUILD_JOBS` is set on the host.
 *
 * | Option | Default | Meaning |
 * |---|---|---|
 * | `--profiles` | `all` | Comma-separated profile names, `quick` or `all`. |
 * | `--scenarios` | `transfer,video` | `transfer`, `video`, `combined`, `combined-download` or `stills`. |
 * | `--seed` | `17` | netem loss seed: the same seed drops the same packet positions. |
 * | `--small-files`, `--small-size` | `1000`, `4096` | The many-small-files case. |
 * | `--large-size` | `1GiB` | The large-file case, e.g. `256MiB` or `10GiB`; the combined scenarios move a file this size. |
 * | `--transfer-timeout` | `120` | Seconds per case from submit; at the deadline the job is cancelled on both sides and reported as far as it got. |
 * | `--driver` | `tight` | `tight` advances the receiver back to back; `background` uses only the app's background job driver. |
 * | `--video-seconds`, `--bursts`, `--burst-ms` | `30`, `5`, `250` | Stream length and the blackouts inside it. |
 * | `--viewer` | `passive` | `passive` is the product viewer; `request-while-corrupt` asks for a keyframe on every corrupt picture. |
 * | `--path` | `direct` | `direct` lets the cores reach each other only directly; `relay` makes every byte cross a local iroh relay. |
 *
 * A case the engine refuses, such as one past its free-space admission, is
 * recorded as `refused` with the engine's error; it is not a harness failure.
 *
 * # Scenarios
 *
 * Each runs as an ignored test of the named crate (`SCENARIOS`), which
 * documents its workload and report; a small version of each runs in
 * ordinary `cargo test` over plain loopback to keep the scenario code
 * working.
 * - `transfer`: the viewer's core uploads 1,000 small files, one large file
 *   and a mixed tree through the job engine; each complete case is compared
 *   byte for byte with its source (`impaired_transfer_matrix`).
 * - `video`: the real shared-capture fanout with a synthetic source and
 *   encoder, blacked out for each burst; reports time to a clean picture
 *   after each burst, keyframes by cause, delivery and clean ratios, viewer
 *   keyframe requests and one-way latency (`impaired_desktop_stream`).
 * - `combined`, `combined-download`: the video scenario while one large file
 *   moves between the cores, uploaded by the viewer or downloaded from the
 *   host, the video's way. Also report the longest freeze and the session's
 *   interruptions and round trip.
 * - `stills`: a screen that alternates still and moving, streamed without
 *   still pictures and then with them.
 *
 * Each case also reports the selected path of every core, once connected and
 * once finished. A run fails unless every core finishes on a connection over
 * the expected path, in either mode.
 *
 * A video scenario also fails when the conditions the viewer held
 * (`vnilink::Condition`) miss the profile's expectation
 * (`expectedConditions`): none on `clean` and `delay-*`; link loss from 2%
 * random or burst loss; congestion held at some point on `rate-2`; a held
 * blackout when `--burst-ms` is 2500 or more; and the relayed path in relay
 * mode, where loss on the relay's TCP leg shows as a relay backlog. `loss-0.5`
 * and `loss-1` sit under the rate controller's loss threshold, so their
 * conditions are recorded, not asserted.
 *
 * # Relay mode
 *
 * `--path relay` measures the relay fallback. `netem.ts` explains how the
 * link carries it. Beyond the path check above, the harness records the
 * relay's evidence:
 * - the relay (`examples/impaired_relay.rs` of `vnilink`) prints cumulative
 *   counters once a second; each run's `relay` entry in `results.json` and
 *   the summary's Relay table give their difference over the run: datagram
 *   frames taken and written, frames dropped for a gone recipient or a
 *   failed write, frames its per-client queues discarded (derived, give or
 *   take one queue), and client connections opened and closed;
 * - once a second `ss -tin` samples every TCP connection to or from the
 *   relay into `<profile>/<scenario>.relay-sockets.jsonl` (Send-Q, TCP round
 *   trip, congestion window, retransmissions); the summary gives each leg's
 *   peak Send-Q and round trip. A backlog queued inside the clients, ahead
 *   of their sockets, shows instead as the QUIC round trip in the transfer
 *   scenario's `files_paths`.
 *
 * The relay is local, on the same host as both engines and over netem: it
 * says nothing about public relays, their load or distance, TLS, or
 * internet paths.
 */
import { mkdir, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { runContainer } from '../containers.ts';
import { isLinux } from '../platform.ts';
import { processOutput, ProcessRunner } from '../processes.ts';
import { parseImpairedArguments, resolveOptions, SCENARIOS, type ResolvedOptions, type ScenarioName } from './impairedOptions.ts';
import { observeRelay, sampleRelaySockets, startRelay, type RelayMonitor } from './impairedRelay.ts';
import { renderSummary, type ImpairedReport, type ScenarioRun } from './impairedReport.ts';
import { conditionMismatch, expectedConditions, pathMismatch, scenarioEnvironment } from './impairedScenarios.ts';
import { applyProfile, createTopology, disposeTopology, linkStatistics, statisticsDelta, topology, writeBurstHooks, type NetemProfile, type Topology } from './netem.ts';

interface RunContext {
	readonly root: string;
	readonly signal: AbortSignal;
	readonly processes: ProcessRunner;
	readonly link: Topology;
	readonly options: ResolvedOptions;
	readonly output: string;
	readonly scratch: string;
	readonly targetDirectory: string;
	/** Relay mode only. */
	relay?: RelayMonitor;
}

async function runScenario(context: RunContext, item: NetemProfile, scenario: ScenarioName): Promise<ScenarioRun> {
	const { root, signal, processes, link, options, output, scratch, targetDirectory, relay } = context;
	const directory = join(output, item.name);
	await mkdir(directory, { recursive: true });
	await mkdir(scratch, { recursive: true });
	const paths = {
		result: join(directory, `${scenario}.json`),
		scratch,
		burstStart: join(directory, 'burst-start.sh'),
		burstEnd: join(directory, 'burst-end.sh'),
	};
	await writeBurstHooks(link, item, options.seed, paths.burstStart, paths.burstEnd);
	const { crate, test } = SCENARIOS[scenario];
	const before = await linkStatistics(link, signal);
	const started = Date.now();
	const run: ScenarioRun = { profile: item.name, scenario, status: 'passed', seconds: 0, result: null, link: null, error: null };
	console.log(`[impaired] ${item.name} ${scenario}`);
	try {
		const args = ['test', '--release', '--locked', '--target-dir', targetDirectory, '-p', crate, '--lib', '--', '--ignored', '--exact', test, '--nocapture'];
		const scenarioRun = processes.run('cargo', args, { cwd: root, env: { ...process.env, ...scenarioEnvironment(scenario, options, paths, link) } });
		if (relay) {
			const counters = relay.latest;
			const sockets = sampleRelaySockets(link, signal, join(directory, `${scenario}.relay-sockets.jsonl`), scenarioRun);
			try {
				await scenarioRun;
			} finally {
				const samples = await sockets;
				// The relay reports once a second; wait for a report made after the run.
				await new Promise(resolve => setTimeout(resolve, 1_100));
				run.relay = observeRelay(counters, relay.latest, samples);
			}
		} else {
			await scenarioRun;
		}
		run.result = JSON.parse(await readFile(paths.result, 'utf8')) as unknown;
		const mismatch = pathMismatch(run.result, options.path);
		if (mismatch) {
			throw new Error(`Wrong transport path: ${mismatch}`);
		}
		// The stills scenario measures changes, not the viewer's conditions over time.
		if (scenario !== 'transfer' && scenario !== 'stills') {
			const wrong = conditionMismatch(run.result, expectedConditions(item.name, options.path, options.burstMs, options.bursts));
			if (wrong) {
				throw new Error(`Wrong condition: ${wrong}`);
			}
		}
	} catch (error) {
		signal.throwIfAborted();
		run.status = 'failed';
		run.error = error instanceof Error ? error.message : String(error);
	}
	run.seconds = (Date.now() - started) / 1000;
	run.link = statisticsDelta(before, await linkStatistics(link, signal));
	if (run.status === 'failed') {
		// A failed scenario may have left its blackout in place.
		await applyProfile(link, item, options.seed, signal);
	}
	return run;
}

/**
 * Builds the scenario tests once, then runs each selected profile and
 * scenario on a fresh link. Needs Linux and root; elsewhere use
 * `measure:impaired`, which runs this in the project container.
 */
export async function measureImpaired(root: string, signal: AbortSignal, args: readonly string[]): Promise<ImpairedReport> {
	if (!isLinux) {
		throw new Error('The impaired-network harness needs Linux network namespaces; run `npm run measure:impaired` to use the container');
	}
	if (process.getuid?.() !== 0) {
		throw new Error('The impaired-network harness needs root to create network namespaces');
	}
	const { output: requested, ...parsed } = parseImpairedArguments(args);
	const options = resolveOptions(parsed);
	const output = requested ?? join(root, '.build/impaired-network', String(Date.now()));
	const processes = new ProcessRunner(signal);
	const context: RunContext = {
		root,
		signal,
		processes,
		link: topology(),
		options,
		output,
		// Fixture trees stay on local disk, outside the evidence directory.
		scratch: join(tmpdir(), `vnilink-impaired-${String(process.pid)}`),
		targetDirectory: process.env.CARGO_TARGET_DIR ?? join(root, 'target/impaired'),
	};
	const report: ImpairedReport = {
		started: new Date().toISOString(),
		commit: process.env.VNILINK_COMMIT ?? null,
		path: options.path,
		options: { ...options, profiles: options.profiles.map(item => item.name) },
		profiles: options.profiles,
		runs: [],
	};
	const crates = [...new Set(options.scenarios.map(name => SCENARIOS[name].crate))];
	const save = () => writeFile(join(output, 'results.json'), `${JSON.stringify(report, null, '\t')}\n`);
	await mkdir(output, { recursive: true });
	try {
		await processes.run('cargo', ['test', '--release', '--locked', '--no-run', '--target-dir', context.targetDirectory, ...crates.flatMap(name => ['-p', name]), '--lib'], { cwd: root });
		if (options.path === 'relay') {
			await processes.run('cargo', ['build', '--release', '--locked', '--target-dir', context.targetDirectory, '-p', 'vnilink', '--example', 'impaired_relay'], { cwd: root });
		}
		await createTopology(context.link, signal, options.path);
		if (options.path === 'relay') {
			context.relay = await startRelay(processes, join(context.targetDirectory, 'release/examples/impaired_relay'), context.link);
		}
		for (const item of options.profiles) {
			await applyProfile(context.link, item, options.seed, signal);
			for (const scenario of options.scenarios) {
				report.runs.push(await runScenario(context, item, scenario));
				await save();
			}
		}
	} finally {
		await processes.stop();
		await disposeTopology(context.link);
		await rm(context.scratch, { recursive: true, force: true });
		await save();
		await writeFile(join(output, 'summary.md'), renderSummary(report));
		console.log(`Impaired-network evidence: ${output}`);
	}
	const failed = report.runs.filter(run => run.status !== 'passed');
	if (failed.length) {
		throw new Error(`${String(failed.length)} impaired-network run(s) failed; see ${join(output, 'summary.md')}`);
	}
	return report;
}

/** Runs `measureImpaired` in a privileged project container; arguments pass through. */
export async function measureImpairedInContainer(root: string, signal: AbortSignal, args: readonly string[]): Promise<void> {
	// Fail on bad arguments before building an image.
	resolveOptions(parseImpairedArguments(args));
	const commit = await processOutput('git', ['rev-parse', 'HEAD'], { cwd: root }).then(stdout => stdout.trim(), () => 'unknown');
	await runContainer(root, signal, {
		purpose: 'impaired',
		evidence: 'impaired-network',
		// Namespaces, veth devices and qdiscs need a privileged container.
		options: [
			'--privileged', '-e', `VNILINK_COMMIT=${commit}`, ...(process.env.CARGO_BUILD_JOBS ? ['-e', `CARGO_BUILD_JOBS=${process.env.CARGO_BUILD_JOBS}`] : []),
			'--entrypoint', '/usr/local/bin/vnilink-impaired',
		],
		args,
		beforeBuild: async processes => {
			// Runs on one machine compete for its CPUs, and both sets of numbers would be wrong.
			const running = (await processes.output('docker', ['ps', '--filter', 'name=vnilink-impaired-', '--format', '{{.Names}}'])).trim();
			if (running) {
				throw new Error(`Another impaired-network run is in progress (${running.split('\n').join(', ')}). Wait for it to finish: runs on one machine skew each other's numbers.`);
			}
		},
	});
}
