import { DisposableStore } from '../../../base/common/lifecycle.js';
import { ProxyChannel } from '../../../base/parts/ipc/common/ipc.js';
import { Server as UtilityProcessServer } from '../../../base/parts/ipc/node/ipc.mp.js';
import { ENGINE_HOST_CHANNEL_NAME } from '../common/engineService.js';
import { EngineHost } from './engineHost.js';

const port = process.parentPort;
// Electron types assume a utility process; reject accidental launches in Node/main.
// eslint-disable-next-line @typescript-eslint/no-unnecessary-condition
if (!port) {
	throw new Error('Engine host requires a parent');
}

const host = new EngineHost(port);

const disposables = new DisposableStore();

const server = disposables.add(new UtilityProcessServer(host));

server.registerChannel(ENGINE_HOST_CHANNEL_NAME, ProxyChannel.fromService({
	execute: (command: unknown) => host.execute(command),
	onDidReceiveEvents: host.onDidReceiveEvents,
}, disposables, { disableMarshalling: true, unbufferedEvents: ['onDidReceiveEvents'] }));

void host.open();
