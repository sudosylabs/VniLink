/**
 * A session with the test agent on another computer
 * (`node build/run.ts testkit:agent` there), for measurements that run one
 * side there: `--peer <file.vnl> --peer-check <digits>`. The session runs
 * the test kit's bridge (`vnilink-testkit bridge`), which pairs with the
 * agent as the application pairs, and drives it over JSON lines: start a
 * scenario compiled into the agent, relay rendezvous files, fetch the
 * scenario's evidence and stop it. A person types the digits it prints on
 * the agent's terminal; nothing is approved unattended.
 */
import { mkdir } from 'node:fs/promises';
import { join, resolve } from 'node:path';
import { createInterface } from 'node:readline';
import { defaultNativeOptions, nativeEnvironment } from './native.ts';
import { isWindows } from './platform.ts';
import { ProcessRunner } from './processes.ts';

/** The agent a run names: its pairing file and the digits printed beside it. */
export interface AgentPeer {
	/** The agent's `.vnl` file, absolute. */
	readonly file: string;
	/** The three digits the agent printed beside it. */
	readonly check: string;
}

/** `--peer` and `--peer-check` as an agent, or undefined when neither is given; one needs the other. */
export function agentPeer(file: string | undefined, check: string | undefined, cwd = process.cwd()): AgentPeer | undefined {
	if (file === undefined && check === undefined) {
		return undefined;
	}
	if (file === undefined || check === undefined) {
		throw new Error('--peer and --peer-check go together: the agent\'s .vnl file and the digits it printed beside it');
	}
	if (!/^\d{3}$/.test(check)) {
		throw new Error('--peer-check takes the three digits the agent printed');
	}
	return { file: resolve(cwd, file), check };
}

/** What the agent's computer runs, as it said when paired: never its name. */
export interface AgentPlatform {
	readonly os: string;
	readonly arch: string;
	readonly profile: string;
}

/** A rendezvous file and its bytes. */
export interface AgentFile {
	readonly name: string;
	readonly data: Buffer;
}

/** The rendezvous files a scenario's side published since the last poll, and whether it still runs. */
export interface AgentPoll {
	readonly files: readonly AgentFile[];
	readonly running: boolean;
}

/** A paired agent, one request at a time. */
export interface AgentSession {
	readonly platform: AgentPlatform;
	/** Starts the scenario `scenario`; the facts its other half needs. */
	start(scenario: string, params?: Readonly<Record<string, unknown>>): Promise<unknown>;
	/** Gives the running scenario's side the rendezvous file `name`. */
	put(name: string, data: Uint8Array): Promise<void>;
	poll(): Promise<AgentPoll>;
	/** Copies every file of the running scenario's output folder under `into`; their paths there. */
	fetch(into: string): Promise<string[]>;
	/** Ends the running scenario; the agent removes what it made. */
	stop(): Promise<void>;
	/** Ends the bridge, and with it the agent's service of this computer. */
	close(): Promise<void>;
}

/** The test kit's command, built with the addon's toolchain as `testkit:agent` builds it, so both sides share one revision. */
async function bridgeExecutable(root: string, signal: AbortSignal): Promise<string> {
	const native = defaultNativeOptions();
	const target = native.target ? ['--target', native.target] : [];
	const processes = new ProcessRunner(signal);
	try {
		await processes.run('cargo', ['build', '--release', '--locked', '--target-dir', 'target/electron', ...target, '-p', 'vnilink-testkit', '--bin', 'vnilink-testkit'], {
			cwd: root,
			env: nativeEnvironment(native),
		});
	} finally {
		await processes.stop();
	}
	return join(root, 'target/electron', ...native.target ? [native.target] : [], 'release', isWindows ? 'vnilink-testkit.exe' : 'vnilink-testkit');
}

/** A line the bridge printed, as JSON. */
type Line = Readonly<Record<string, unknown>>;

const text = (value: unknown): string => typeof value === 'string' ? value : JSON.stringify(value);

/** The code of the error thrown when the bridge answers out of its protocol: a line that is no JSON object, or no line at all. */
export const BRIDGE_PROTOCOL = 'BRIDGE_PROTOCOL';

function protocolError(message: string): Error {
	return Object.assign(new Error(message), { code: BRIDGE_PROTOCOL });
}

/** Collaborators a test replaces. */
export interface AgentSessionOptions {
	/** The bridge to run in place of the test kit's, built from this checkout. */
	readonly bridge?: { readonly command: string; readonly args: readonly string[] };
}

/**
 * Pairs with the agent `peer` names: prints the digits to type on the
 * agent's terminal and resolves once the agent's owner typed them. `signal`
 * cancels the pairing only: once paired, the session outlives it, so a run
 * that is interrupted can still stop its scenario on the agent; the caller
 * closes the session in every case.
 */
export async function openAgentSession(root: string, peer: AgentPeer, signal: AbortSignal, options: AgentSessionOptions = {}): Promise<AgentSession> {
	const command = options.bridge ?? { command: await bridgeExecutable(root, signal), args: [] };
	// Its own owner, apart from `signal`: see above.
	const processes = new ProcessRunner();
	const bridge = processes.start(command.command, [...command.args, 'bridge', '--peer', peer.file, '--peer-check', peer.check], { stdio: ['pipe', 'pipe', 'inherit'] });
	const { stdin, stdout } = bridge.child;
	if (!stdin || !stdout) {
		await processes.stop().catch(() => undefined);
		throw new Error('The bridge has no input or output');
	}
	const lines = createInterface({ input: stdout })[Symbol.asyncIterator]();
	const next = async (): Promise<Line> => {
		const line = await lines.next();
		if (line.done) {
			throw protocolError('The bridge to the agent ended');
		}
		let value: unknown;
		try {
			value = JSON.parse(line.value);
		} catch {
			value = undefined;
		}
		if (typeof value !== 'object' || value === null || Array.isArray(value)) {
			throw protocolError(`The bridge printed something other than a JSON object: ${line.value.slice(0, 200)}`);
		}
		return value as Line;
	};
	const paired = async (): Promise<AgentPlatform> => {
		const approve = await next();
		if (approve.event !== 'approve') {
			throw new Error(`The agent could not be paired with: ${text(approve.error)}`);
		}
		console.log(`Type ${text(approve.digits)} on the agent's terminal to approve this computer.`);
		const ready = await next();
		if (ready.event !== 'ready') {
			throw new Error(`The agent could not be paired with: ${text(ready.error)}`);
		}
		return ready.platform as AgentPlatform;
	};
	let platform: AgentPlatform;
	let onAbort: (() => void) | undefined;
	try {
		signal.throwIfAborted();
		const aborted = new Promise<never>((_resolve, reject) => {
			onAbort = () => {
				reject(signal.reason instanceof Error ? signal.reason : new Error('Interrupted while pairing with the agent'));
			};
			signal.addEventListener('abort', onAbort, { once: true });
		});
		aborted.catch(() => undefined);
		platform = await Promise.race([paired(), aborted]);
	} catch (error) {
		// The pairing's own error is the one to report, not the teardown's.
		await processes.stop().catch(() => undefined);
		throw error;
	} finally {
		if (onAbort) {
			signal.removeEventListener('abort', onAbort);
		}
	}
	// One request at a time, in order: each line answers the one before.
	let queue: Promise<unknown> = Promise.resolve();
	const ask = (command: Readonly<Record<string, unknown>>): Promise<Line> => {
		const answer = queue.then(async () => {
			stdin.write(`${JSON.stringify(command)}\n`);
			const reply = await next();
			if (reply.ok !== true) {
				throw new Error(`The agent refused ${text(command.op)}: ${text(reply.error)}`);
			}
			return reply;
		});
		queue = answer.catch(() => undefined);
		return answer;
	};
	return {
		platform,
		start: async (scenario, params = {}) => (await ask({ op: 'start', scenario, params })).facts,
		put: async (name, data) => {
			await ask({ op: 'put', name, data: Buffer.from(data).toString('base64') });
		},
		poll: async () => {
			const reply = await ask({ op: 'poll' });
			const files = Array.isArray(reply.files) ? reply.files as { name: string; data: string }[] : [];
			return { files: files.map(file => ({ name: file.name, data: Buffer.from(file.data, 'base64') })), running: reply.running === true };
		},
		fetch: async into => {
			await mkdir(into, { recursive: true });
			const reply = await ask({ op: 'fetch', into: resolve(into) });
			return Array.isArray(reply.files) ? reply.files.map(text) : [];
		},
		stop: async () => {
			await ask({ op: 'stop' });
		},
		close: async () => {
			await queue;
			stdin.end();
			let timer: NodeJS.Timeout | undefined;
			try {
				await Promise.race([bridge.result.catch(() => undefined), new Promise(resolve => {
					timer = setTimeout(resolve, 10000);
				})]);
			} finally {
				clearTimeout(timer);
				await processes.stop();
			}
		},
	};
}
