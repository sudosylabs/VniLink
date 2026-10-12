import { createRequire } from 'node:module';
import { join } from 'node:path';
import { record } from '../common/engine.js';

interface NativeEngine { execute(command: string): Promise<string>; }
interface Addon { openEngine(): Promise<NativeEngine>; openTemporaryEngine(): Promise<NativeEngine>; }

function isAddon(value: unknown): value is Addon {
	return record(value) && typeof value.openEngine === 'function' && typeof value.openTemporaryEngine === 'function';
}

export class EngineHost {
	private engine: NativeEngine | undefined;
	constructor(private readonly port: Electron.ParentPort) { }
	public listen(): void {
		this.port.on('message', (event: { data: unknown }) => {
			this.onMessage(event.data);
		});
	}
	public async open(): Promise<void> {
		try {
			const addon: unknown = createRequire(__filename)(join(__dirname, 'native', 'vnilink.node'));
			if (!isAddon(addon)) {
				throw new Error('Invalid addon');
			}
			this.engine = await (process.argv.includes('--temporary') ? addon.openTemporaryEngine() : addon.openEngine());
			this.port.postMessage({ ready: true });
		} catch {
			this.port.postMessage({ ready: false }); process.exit(1);
		}
	}
	private onMessage(message: unknown): void {
		// Shape-checked only: command validation lives at the IPC boundary and in
		// Rust, so this host never kills the process over a bad caller. Unknown
		// ids are ignored; known ids get a per-operation error instead.
		if (!record(message) || typeof message.id !== 'number' || !Number.isSafeInteger(message.id) || message.id <= 0) {
			return;
		}
		const id = message.id;
		const command = message.command;
		const shutdown = record(command) && command.type === 'shutdown' && Object.keys(command).length === 1;
		if (!record(command)) {
			this.port.postMessage({ id, error: 'Invalid engine command' }); return;
		}
		void this.execute(id, command, shutdown);
	}
	private async execute(id: number, command: Record<string, unknown>, shutdown: boolean): Promise<void> {
		try {
			if (!this.engine) {
				throw new Error('Engine unavailable');
			}
			// The reply is validated by the main-process queue; forward it
			// untouched so only one boundary parses its shape.
			const reply: unknown = JSON.parse(await this.engine.execute(JSON.stringify(command)));
			this.port.postMessage({ id, reply });
			if (shutdown) {
				process.exit(0);
			}
		} catch (error) {
			// Native error strings name the reason only, never secrets.
			const reason = error instanceof Error && error.message.length > 0 ? error.message : 'Engine operation failed';
			this.port.postMessage({ id, error: reason.slice(0, 512) });
		}
	}
}

const port = process.parentPort;
// Electron types assume a utility process; reject accidental launches in Node/main.
// eslint-disable-next-line @typescript-eslint/no-unnecessary-condition
if (!port) {
	throw new Error('Engine host requires a parent');
}
const host = new EngineHost(port);
host.listen();
void host.open();
