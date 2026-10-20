/** Network impairment profiles and the two-namespace veth link that applies them. */
import { execFile } from 'node:child_process';
import { chmod, writeFile } from 'node:fs/promises';
import { promisify } from 'node:util';

const run = promisify(execFile);

export type Loss =
	| { readonly kind: 'random'; readonly percent: number }
	| { readonly kind: 'gemodel'; readonly p: number; readonly r: number; readonly h: number; readonly k: number };

export interface NetemProfile {
	readonly name: string;
	readonly description: string;
	readonly rttMs?: number;
	readonly jitterMs?: number;
	readonly loss?: Loss;
	readonly rateMbit?: number;
}

function profile(name: string, description: string, impairment: Omit<NetemProfile, 'name' | 'description'>): NetemProfile {
	return Object.freeze({ name, description, ...impairment });
}

function random(percent: number): Loss {
	return { kind: 'random', percent };
}

/**
 * Each impairment applies to both directions. Delays are round trips split
 * evenly across the two directions. Every impaired profile keeps at least a
 * 20 ms round trip so loss recovery runs at a realistic RTT instead of
 * loopback speed. Jitter uses netem slots, which hold and release packets in
 * bursts without reordering them; netem's own jitter would reorder packets
 * and turn a delay profile into a reordering test.
 */
export const NETEM_PROFILES: readonly NetemProfile[] = Object.freeze([
	profile('clean', 'No impairment: the veth link as is.', {}),
	profile('loss-0.5', '0.5% random loss, 20 ms RTT.', { rttMs: 20, loss: random(0.5) }),
	profile('loss-1', '1% random loss, 20 ms RTT.', { rttMs: 20, loss: random(1) }),
	profile('loss-2', '2% random loss, 20 ms RTT.', { rttMs: 20, loss: random(2) }),
	profile('loss-5', '5% random loss, 20 ms RTT.', { rttMs: 20, loss: random(5) }),
	profile('loss-burst', 'Gilbert-Elliott bursts: about 2% loss in runs of about 4 packets, 20 ms RTT.', { rttMs: 20, loss: { kind: 'gemodel', p: 0.5, r: 25, h: 100, k: 0 } }),
	profile('delay-20', '20 ms RTT with up to 4 ms of jitter.', { rttMs: 20, jitterMs: 4 }),
	profile('delay-50', '50 ms RTT with up to 10 ms of jitter.', { rttMs: 50, jitterMs: 10 }),
	profile('delay-100', '100 ms RTT with up to 20 ms of jitter.', { rttMs: 100, jitterMs: 20 }),
	profile('rate-2', '2 Mb/s each way, 20 ms RTT, about 250 ms of queue.', { rttMs: 20, rateMbit: 2 }),
	profile('rate-5', '5 Mb/s each way, 20 ms RTT, about 250 ms of queue.', { rttMs: 20, rateMbit: 5 }),
	profile('rate-10', '10 Mb/s each way, 20 ms RTT, about 250 ms of queue.', { rttMs: 20, rateMbit: 10 }),
]);

/** A short run that still covers each impairment kind. */
export const QUICK_PROFILES: readonly string[] = Object.freeze(['clean', 'loss-1', 'loss-5', 'loss-burst', 'delay-50', 'rate-5']);

const MTU_BITS = 1500 * 8;

const RATE_QUEUE_SECONDS = 0.25;

// Unshaped profiles must never drop for want of queue: at link speed a
// delayed direction holds a whole bandwidth-delay product.
const UNSHAPED_LIMIT = 100_000;

// The engines send UDP GSO batches, which reach a root qdisc as one packet
// of up to 64 KiB, so netem would drop and pace whole batches. A token bucket
// fast enough never to shape splits any packet larger than its 2000-byte
// burst into wire-sized segments before they reach the netem child.
const SEGMENTING_ROOT = Object.freeze(['root', 'handle', '1:', 'tbf', 'rate', '10gbit', 'burst', '2000', 'latency', '1s']);

const NETEM_CHILD = Object.freeze(['parent', '1:1', 'handle', '10:', 'netem']);

const ROUTE_TABLE = '100';

/** Resolves names, `quick` and `all` in order, rejecting unknown names. */
export function selectProfiles(names: readonly string[]): NetemProfile[] {
	const selected: NetemProfile[] = [];
	for (const name of names) {
		const expanded = name === 'all' ? NETEM_PROFILES.map(item => item.name) : name === 'quick' ? QUICK_PROFILES : [name];
		for (const each of expanded) {
			const found = NETEM_PROFILES.find(item => item.name === each);
			if (!found) {
				throw new Error(`Unknown network profile: ${each}. Known profiles: ${NETEM_PROFILES.map(item => item.name).join(', ')}, quick, all`);
			}
			if (!selected.includes(found)) {
				selected.push(found);
			}
		}
	}
	if (!selected.length) {
		throw new Error('Select at least one network profile');
	}
	return selected;
}

function milliseconds(value: number): string {
	return `${String(value)}ms`;
}

function percent(value: number): string {
	return `${String(value)}%`;
}

/** `tc ... netem` arguments for one direction of `item`. */
export function netemArguments(item: NetemProfile, seed: number): string[] {
	const args: string[] = [];
	const oneWay = (item.rttMs ?? 0) / 2;
	if (oneWay) {
		args.push('delay', milliseconds(oneWay));
	}
	if (item.jitterMs) {
		args.push('slot', '0ms', milliseconds(item.jitterMs / 2));
	}
	// Loss is always explicit: a blackout hook replaces it in place, and netem
	// keeps any parameter a replacement leaves out.
	if (item.loss?.kind === 'gemodel') {
		args.push('loss', 'gemodel', percent(item.loss.p), percent(item.loss.r), percent(item.loss.h), percent(item.loss.k));
	} else {
		args.push('loss', 'random', percent(item.loss?.percent ?? 0));
	}
	if (item.rateMbit) {
		args.push('rate', `${String(item.rateMbit)}mbit`);
	}
	const limit = item.rateMbit ? Math.max(20, Math.ceil(item.rateMbit * 1e6 * (RATE_QUEUE_SECONDS + oneWay / 1000) / MTU_BITS)) : UNSHAPED_LIMIT;
	args.push('limit', String(limit), 'seed', String(seed));
	return args;
}

/** The same link with every packet dropped, for a loss burst. */
export function blackoutArguments(item: NetemProfile): string[] {
	return netemArguments({ ...item, loss: random(100) }, 1);
}

function quote(value: string): string {
	return `'${value.replaceAll('\'', '\'\\\'\'')}'`;
}

/** A POSIX shell script that runs `commands` in order and stops at the first failure. */
export function shellScript(commands: readonly (readonly string[])[]): string {
	return ['#!/bin/sh', 'set -e', ...commands.map(command => command.map(quote).join(' ')), ''].join('\n');
}

export interface Topology {
	readonly names: { readonly host: string; readonly view: string };
	readonly sides: readonly { readonly namespace: string; readonly device: string }[];
}

/**
 * Two namespaces joined by one veth pair. Each namespace keeps 127.0.0.1 on
 * loopback, while policy routing sends locally originated loopback traffic
 * out of the veth, so engines bound to 127.0.0.1 in different namespaces
 * reach each other through the impaired link. Received packets still match
 * the local table and are delivered to the bound socket.
 */
export function topology(names = { host: 'vnl-host', view: 'vnl-view' }): Topology {
	return Object.freeze({
		names: Object.freeze({ ...names }),
		sides: Object.freeze([
			Object.freeze({ namespace: names.host, device: `${names.host}-veth` }),
			Object.freeze({ namespace: names.view, device: `${names.view}-veth` }),
		]),
	});
}

/** Installs fresh qdiscs, so nothing from the previous profile carries over. */
export function profileCommands(link: Topology, args: readonly string[]): string[][] {
	return link.sides.flatMap(side => [
		['tc', '-n', side.namespace, 'qdisc', 'del', 'dev', side.device, 'root'],
		['tc', '-n', side.namespace, 'qdisc', 'add', 'dev', side.device, ...SEGMENTING_ROOT],
		['tc', '-n', side.namespace, 'qdisc', 'add', 'dev', side.device, ...NETEM_CHILD, ...args],
	]);
}

/** Changes both directions' netem settings in place, keeping queued packets. Parameters left out keep their values. */
export function qdiscCommands(link: Topology, args: readonly string[]): string[][] {
	return link.sides.map(side => ['tc', '-n', side.namespace, 'qdisc', 'replace', 'dev', side.device, ...NETEM_CHILD, ...args]);
}

/** Commands that create the link. The neighbour entries follow once the device addresses are known. */
export function creationCommands(link: Topology): string[][] {
	const [host, view] = link.sides;
	const commands = [
		['ip', 'netns', 'add', host.namespace],
		['ip', 'netns', 'add', view.namespace],
		['ip', 'link', 'add', host.device, 'type', 'veth', 'peer', 'name', view.device],
		['ip', 'link', 'set', host.device, 'netns', host.namespace],
		['ip', 'link', 'set', view.device, 'netns', view.namespace],
	];
	for (const side of link.sides) {
		const inside = ['ip', '-n', side.namespace];
		commands.push(
			[...inside, 'link', 'set', 'lo', 'up'],
			[...inside, 'link', 'set', side.device, 'mtu', '1500', 'up'],
			['ip', 'netns', 'exec', side.namespace, 'sysctl', '-q', '-w', 'net.ipv4.conf.all.rp_filter=0', `net.ipv4.conf.${side.device}.rp_filter=0`, `net.ipv4.conf.${side.device}.route_localnet=1`, `net.ipv4.conf.${side.device}.accept_local=1`],
			[...inside, 'route', 'add', '127.0.0.1/32', 'dev', side.device, 'table', ROUTE_TABLE, 'scope', 'link'],
			[...inside, 'rule', 'add', 'pref', '10', 'iif', 'lo', 'lookup', ROUTE_TABLE],
			[...inside, 'rule', 'add', 'pref', '20', 'lookup', 'local'],
			[...inside, 'rule', 'del', 'pref', '0'],
		);
	}
	// A new device has no root qdisc to delete.
	return [...commands, ...profileCommands(link, ['limit', String(UNSHAPED_LIMIT)]).filter(command => command[4] !== 'del')];
}

async function runAll(commands: readonly (readonly string[])[], signal: AbortSignal): Promise<void> {
	for (const [command, ...args] of commands) {
		signal.throwIfAborted();
		await run(command, args, { signal });
	}
}

async function json(command: string, args: readonly string[], signal: AbortSignal): Promise<Record<string, unknown>[]> {
	return JSON.parse((await run(command, args, { signal })).stdout) as Record<string, unknown>[];
}

/** Creates the link, replacing any namespaces left behind by an interrupted run. */
export async function createTopology(link: Topology, signal: AbortSignal): Promise<void> {
	await disposeTopology(link);
	await runAll(creationCommands(link), signal);
	const [host, view] = link.sides;
	const address = async (side: Topology['sides'][number]) => String((await json('ip', ['-n', side.namespace, '-j', 'link', 'show', 'dev', side.device], signal))[0].address);
	const hostAddress = await address(host);
	const viewAddress = await address(view);
	await run('ip', ['-n', host.namespace, 'neigh', 'replace', '127.0.0.1', 'lladdr', viewAddress, 'dev', host.device, 'nud', 'permanent'], { signal });
	await run('ip', ['-n', view.namespace, 'neigh', 'replace', '127.0.0.1', 'lladdr', hostAddress, 'dev', view.device, 'nud', 'permanent'], { signal });
}

export async function applyProfile(link: Topology, item: NetemProfile, seed: number, signal: AbortSignal): Promise<void> {
	await runAll(profileCommands(link, netemArguments(item, seed)), signal);
}

/** Writes executable hooks that start and end a blackout on top of `item`; only the loss changes. */
export async function writeBurstHooks(link: Topology, item: NetemProfile, seed: number, startPath: string, endPath: string): Promise<void> {
	await writeFile(startPath, shellScript(qdiscCommands(link, blackoutArguments(item))));
	await writeFile(endPath, shellScript(qdiscCommands(link, netemArguments(item, seed))));
	await chmod(startPath, 0o755);
	await chmod(endPath, 0o755);
}

export type LinkCounters = Record<string, Record<string, number>>;

/** Device counters, which may count GSO batches, and netem counters, which count wire-sized segments. */
export async function linkStatistics(link: Topology, signal: AbortSignal): Promise<LinkCounters> {
	const sides: LinkCounters = {};
	for (const side of link.sides) {
		const device = (await json('ip', ['-n', side.namespace, '-s', '-j', 'link', 'show', 'dev', side.device], signal))[0] as { stats64: Record<'tx' | 'rx', { packets: number; bytes: number }> };
		const netem = (await json('tc', ['-n', side.namespace, '-s', '-j', 'qdisc', 'show', 'dev', side.device], signal)).find(item => item.kind === 'netem') as { packets: number; drops: number } | undefined;
		sides[side.namespace] = {
			txPackets: device.stats64.tx.packets,
			txBytes: device.stats64.tx.bytes,
			rxPackets: device.stats64.rx.packets,
			rxBytes: device.stats64.rx.bytes,
			netemPackets: netem?.packets ?? 0,
			netemDrops: netem?.drops ?? 0,
		};
	}
	return sides;
}

/** Deleting the namespaces also deletes the veth pair. Missing namespaces are not an error, and teardown ignores cancellation. */
export async function disposeTopology(link: Topology): Promise<void> {
	for (const side of link.sides) {
		await run('ip', ['netns', 'del', side.namespace]).catch(() => undefined);
	}
}

/** Per-counter difference between two `linkStatistics()` samples. */
export function statisticsDelta(before: LinkCounters, after: LinkCounters): LinkCounters {
	return Object.fromEntries(Object.entries(after).map(([namespace, counters]) => {
		const previous = Object.hasOwn(before, namespace) ? before[namespace] : {};
		const difference = ([key, value]: [string, number]) => [key, value - (Object.hasOwn(previous, key) ? previous[key] : 0)];
		return [namespace, Object.fromEntries(Object.entries(counters).map(difference))];
	}));
}
