import type { DesktopApi } from '../../../../platform/ipc/common/desktopApi.js';
import { ConnectionReadbackError, type IConnectionService, type ConnectionSettings, type ConnectionUpdate } from '../common/connectionService.js';
import { validateRelays } from '../common/relayValidation.js';

export class ConnectionService implements IConnectionService {
	declare public readonly _serviceBrand: undefined;
	private pending: Promise<unknown> = Promise.resolve();
	constructor(private readonly api: Pick<DesktopApi, 'engineCommand'>, public readonly available: boolean) { }
	public async load(): Promise<ConnectionSettings> {
		if (!this.available) {
			throw new Error('Connection service unavailable');
		}
		const [saved, active, restart] = await Promise.all([
			this.api.engineCommand({ type: 'savedRelays' }),
			this.api.engineCommand({ type: 'snapshot' }),
			this.api.engineCommand({ type: 'relayRestartRequired' }),
		]);
		if (saved.type !== 'relays' || active.type !== 'snapshot' || restart.type !== 'restartRequired') {
			throw new Error('Invalid connection settings reply');
		}
		return {
			restartRequired: restart.required,
			saved: Object.freeze(saved.relays.map(relay => Object.freeze({ ...relay }))),
			active: Object.freeze({ publicRelay: active.snapshot.public_relay, urls: Object.freeze([...active.snapshot.relay_urls]) }),
		};
	}
	public save(update: ConnectionUpdate): Promise<ConnectionSettings> {
		// Capture drafts so queued saves cannot change underneath the writer.
		const draft: ConnectionUpdate = update.mode === 'public' ? { mode: 'public' } : { mode: 'custom', relays: update.relays.map(relay => ({ ...relay })) };
		const result = this.pending.then(async () => {
			const current = await this.load();
			if (draft.mode === 'custom') {
				const issues = validateRelays(draft.relays, new Set(current.saved.filter(relay => relay.authenticated).map(relay => relay.url)));
				if (issues.length) {
					throw new Error(issues[0].message);
				}
			}
			const reply = await this.api.engineCommand(draft.mode === 'public' ? { type: 'setRelayPublic' } : { type: 'saveCustomRelays', relays: [...draft.relays] });
			if (reply.type !== 'done') {
				throw new Error('Invalid relay save reply');
			}
			try {
				return await this.load();
			} catch {
				throw new ConnectionReadbackError();
			}
		});
		this.pending = result.catch(() => undefined);
		return result;
	}
}
