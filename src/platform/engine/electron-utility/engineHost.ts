import { createRequire } from 'node:module';
import { join } from 'node:path';
import { isEngineCommand, isEngineReply, record } from '../common/engine.js';
interface NativeEngine { execute(command: string): Promise<string>; }
interface Addon { openEngine(): Promise<NativeEngine>; openTemporaryEngine(): Promise<NativeEngine>; }
function isAddon(value: unknown): value is Addon {
	return record(value) && typeof value.openEngine === 'function' && typeof value.openTemporaryEngine === 'function';
}
const port = process.parentPort;
// Electron types assume a utility process; reject accidental launches in Node/main.
// eslint-disable-next-line @typescript-eslint/no-unnecessary-condition
if (!port) {
	throw new Error('Engine host requires a parent');
}
let engine: NativeEngine | undefined;
let busy = false;
port.on('message', (event: { data: unknown }) => {
	const message = event.data;
	if (!record(message) || !Number.isSafeInteger(message.id) || typeof message.id !== 'number' || message.id <= 0 || busy) {
		process.exit(1);
	}
	const command = message.command;
	const shutdown = record(command) && command.type === 'shutdown' && Object.keys(command).length === 1;
	if (!shutdown && !isEngineCommand(command)) {
		port.postMessage({ id: message.id, error: 'Invalid engine command' }); return;
	}
	busy = true;
	void (async () => {
		try {
			if (!engine) {
				throw new Error('Engine unavailable');
			}
			const reply: unknown = JSON.parse(await engine.execute(JSON.stringify(command)));
			if (!isEngineReply(reply)) {
				throw new Error('Invalid engine reply');
			}
			port.postMessage({ id: message.id, reply });
			if (shutdown) {
				process.exit(0);
			}
		} catch {
			// Arguments and native error strings can contain pairing credentials.
			port.postMessage({ id: message.id, error: 'Engine operation failed' });
		} finally {
			busy = false;
		}
	})();
});
void (async () => {
	try {
		const addon: unknown = createRequire(__filename)(join(__dirname, 'native', 'vnilink.node'));
		if (!isAddon(addon)) {
			throw new Error('Invalid addon');
		}
		engine = await (process.argv.includes('--temporary') ? addon.openTemporaryEngine() : addon.openEngine());
		port.postMessage({ ready: true });
	} catch {
		port.postMessage({ ready: false }); process.exit(1);
	}
})();
