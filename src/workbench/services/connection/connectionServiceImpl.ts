import { InstantiationType, registerSingleton } from '../../../platform/instantiation/common/extensions.js';
import { IEngineService } from '../../../platform/engine/common/engineService.js';
import { ConnectionReadbackError, IConnectionService, type ConnectionSettings, type ConnectionUpdate } from './connectionService.js';
import { validateRelays } from './relayValidation.js';

export class ConnectionService implements IConnectionService {
	declare public readonly _serviceBrand: undefined;
	private pending: Promise<unknown> = Promise.resolve();

	constructor(@IEngineService private readonly engine: IEngineService) { }

	public get available(): boolean {
		return this.engine.available;
	}

	public async load(): Promise<ConnectionSettings> {
		if (!this.available) {
			throw new Error('Connection service unavailable');
		}
		const [saved, active, restart] = await Promise.all([
			this.engine.execute({ type: 'savedRelays' }),
			this.engine.execute({ type: 'snapshot' }),
			this.engine.execute({ type: 'relayRestartRequired' }),
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
			const reply = await this.engine.execute(draft.mode === 'public' ? { type: 'setRelayPublic' } : { type: 'saveCustomRelays', relays: [...draft.relays] });
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

registerSingleton(IConnectionService, ConnectionService, InstantiationType.Delayed);
