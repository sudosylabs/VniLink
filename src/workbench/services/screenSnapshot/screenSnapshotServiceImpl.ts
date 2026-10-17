import { Disposable } from '../../../base/common/lifecycle.js';
import { DataChannelClient } from '../../../platform/engine/electron-browser/dataChannelClient.js';
import { IEngineService } from '../../../platform/engine/common/engineService.js';
import type { EngineReply, SessionInfo } from '../../../platform/engine/common/engine.js';
import { parseScreenAccess, parseScreenSnapshot } from './screenSnapshot.js';
import type { ScreenAccess, ScreenAccessOutcome, ScreenSnapshotResult } from './screenSnapshot.js';
import type { IScreenSnapshotService } from './screenSnapshotService.js';

function requestStopped(signal: AbortSignal, disposed: boolean): boolean {
	return signal.aborted || disposed;
}

interface SnapshotMedia { readonly type: 'snapshot'; readonly prompt: boolean; readonly access: boolean }

export class ScreenSnapshotService extends Disposable implements IScreenSnapshotService {
	declare public readonly _serviceBrand: undefined;

	constructor(@IEngineService private readonly engine: IEngineService) {
		super();
	}

	public async capture(endpointId: string, signal: AbortSignal, prompt: boolean): Promise<ScreenSnapshotResult> {
		const found = await this.sessionFor(endpointId, signal);
		if (!found.ok) {
			return { status: found.status };
		}
		const packet = await this.packet(found.session, signal, { type: 'snapshot', prompt, access: false });
		if (packet === 'cancelled' || packet === 'failed') {
			return { status: packet };
		}
		const parsed = parseScreenSnapshot(packet.metadata, packet.data) ?? { status: 'failed' as const };
		if (parsed.status === 'ready' && (parsed.endpointId !== found.session.peer_id || parsed.sessionId !== found.session.id || parsed.generation !== found.session.generation)) {
			return { status: 'failed' };
		}
		return parsed;
	}

	public async access(endpointId: string, signal: AbortSignal): Promise<ScreenAccess> {
		const found = await this.sessionFor(endpointId, signal);
		if (!found.ok) {
			return { status: found.status };
		}
		const packet = await this.packet(found.session, signal, { type: 'snapshot', prompt: false, access: true });
		if (packet === 'cancelled' || packet === 'failed') {
			return { status: packet };
		}
		return this.accessResult(parseScreenAccess(packet.metadata, packet.data), found.session);
	}

	private isDisposed(): boolean {
		return this._store.isDisposed;
	}

	private accessResult(outcome: ScreenAccessOutcome | undefined, session: SessionInfo): ScreenAccess {
		if (outcome === 'allowed') {
			return { status: 'allowed', endpointId: session.peer_id, sessionId: session.id, generation: session.generation };
		}
		if (outcome === 'sharingDisabled') {
			return { status: 'denied' };
		}
		if (outcome === 'unavailable' || outcome === 'disconnected' || outcome === 'replaced' || outcome === 'unpaired' || outcome === 'unsupported' || outcome === 'timedOut' || outcome === 'busy' || outcome === 'failed' || outcome === 'cancelled') {
			return { status: outcome };
		}
		return { status: 'failed' };
	}

	private async sessionFor(endpointId: string, signal: AbortSignal): Promise<{ ok: true; session: SessionInfo } | { ok: false; status: 'cancelled' | 'failed' | 'disconnected' }> {
		if (signal.aborted || this.isDisposed()) {
			return { ok: false, status: 'cancelled' };
		}
		if (!this.engine.available) {
			return { ok: false, status: 'failed' };
		}
		let reply: EngineReply;
		try {
			reply = await this.engine.execute({ type: 'controllerSessions' });
		} catch {
			return { ok: false, status: requestStopped(signal, this.isDisposed()) ? 'cancelled' : 'failed' };
		}
		if (requestStopped(signal, this.isDisposed())) {
			return { ok: false, status: 'cancelled' };
		}
		if (reply.type !== 'sessions') {
			return { ok: false, status: 'failed' };
		}
		const session = reply.sessions.find(item => item.role === 'Controller' && item.peer_id === endpointId && item.phase === 'Active');
		if (!session) {
			return { ok: false, status: 'disconnected' };
		}
		return { ok: true, session };
	}

	private packet(session: SessionInfo, signal: AbortSignal, media: SnapshotMedia): Promise<{ metadata: string; data?: Uint8Array } | 'cancelled' | 'failed'> {
		return new Promise(resolve => {
			let settled = false;
			let client: DataChannelClient | undefined;
			const finish = (result: { metadata: string; data?: Uint8Array } | 'cancelled' | 'failed'): void => {
				if (settled) {
					return;
				}
				settled = true;
				signal.removeEventListener('abort', onAbort);
				client?.close();
				resolve(result);
			};
			const onAbort = (): void => {
				finish('cancelled');
			};
			signal.addEventListener('abort', onAbort, { once: true });
			if (signal.aborted) {
				finish('cancelled');
				return;
			}
			void DataChannelClient.open(this.engine, {
				kind: 'snapshot',
				sessionId: session.id,
				generation: session.generation,
				media,
			}, packet => {
				if (signal.aborted || this.isDisposed()) {
					finish('cancelled');
					return;
				}
				if (packet.kind !== 'snapshot') {
					finish('failed');
					return;
				}
				finish({ metadata: packet.metadata, data: packet.data });
			}, () => {
				finish(signal.aborted || this.isDisposed() ? 'cancelled' : 'failed');
			}).then(opened => {
				client = opened;
				if (settled) {
					opened.close();
				}
			}, () => {
				finish(signal.aborted || this.isDisposed() ? 'cancelled' : 'failed');
			});
		});
	}
}
