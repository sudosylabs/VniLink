import { InstantiationType, registerSingleton } from '../../../platform/instantiation/common/extensions.js';
import { IEngineService } from '../../../platform/engine/common/engineService.js';
import type { QualityLimits, DisplayInfo } from '../../../platform/engine/common/engine.js';
import { DesktopReadbackError, desktopTargetKey, isValidDesktopQuality, type DesktopSession, type DesktopSettings, type DesktopTarget, IRemoteDesktopService } from './remoteDesktopService.js';

export class RemoteDesktopService implements IRemoteDesktopService {
	declare public readonly _serviceBrand: undefined;
	private pending: Promise<unknown> = Promise.resolve();

	constructor(@IEngineService private readonly engine: IEngineService) { }

	public get available(): boolean {
		return this.engine.available;
	}

	public async load(): Promise<DesktopSettings> {
		if (!this.available) {
			throw new Error('Desktop engine unavailable');
		}
		const [controllers, hosts, snapshot, codecs] = await Promise.all([
			this.engine.execute({ type: 'controllerSessions' }), this.engine.execute({ type: 'hostedSessions' }), this.engine.execute({ type: 'snapshot' }), this.engine.execute({ type: 'videoCodecs' }),
		]);
		if (controllers.type !== 'sessions' || hosts.type !== 'sessions' || snapshot.type !== 'snapshot' || codecs.type !== 'videoCodecs') {
			throw new Error('Invalid desktop settings reply');
		}
		const sessions: DesktopSession[] = [];
		for (const session of [...controllers.sessions, ...hosts.sessions]) {
			if (session.phase !== 'Active' || !session.quality || !isValidDesktopQuality(session.quality)) {
				continue;
			}
			const peer = snapshot.snapshot.devices.find(device => device.endpoint_id === session.peer_id);
			const name = peer?.pet_name ?? peer?.hostname ?? session.peer_id;
			const quality = Object.freeze({ ...session.quality });
			sessions.push(Object.freeze({ id: session.id, generation: session.generation, role: session.role, name, quality }));
		}
		return { sessions: Object.freeze(sessions), codecs: Object.freeze({ ...codecs.codecs }) };
	}

	private async target(target: DesktopTarget): Promise<DesktopSession> {
		const session = (await this.load()).sessions.find(session => desktopTargetKey(session) === desktopTargetKey(target));
		if (!session) {
			throw new Error('Desktop session ended or reconnected');
		}
		return session;
	}

	public async displays(target: DesktopTarget): Promise<readonly DisplayInfo[]> {
		await this.target(target);
		const reply = await this.engine.execute(target.role === 'Controller' ? { type: 'sessionTopology', sessionId: target.id, generation: target.generation } : { type: 'topology' });
		if (reply.type !== 'displays') {
			throw new Error('Invalid display reply');
		}
		return Object.freeze(reply.displays.map(display => Object.freeze({ ...display })));
	}

	public apply(target: DesktopTarget, quality: QualityLimits): Promise<DesktopSettings> {
		const owner = { ...target };
		const draft = { ...quality };
		const result = this.pending.then(async () => {
			if (!isValidDesktopQuality(draft)) {
				throw new Error('Invalid stream limits');
			}
			await this.target(owner);
			const reply = await this.engine.execute({ type: 'updateSessionQuality', sessionId: owner.id, generation: owner.generation, role: owner.role, quality: draft });
			if (reply.type !== 'done') {
				throw new Error('Invalid quality update reply');
			}
			try {
				const settings = await this.load();
				if (!settings.sessions.some(session => desktopTargetKey(session) === desktopTargetKey(owner))) {
					throw new Error('Session disappeared');
				}
				return settings;
			} catch {
				throw new DesktopReadbackError();
			}
		});
		this.pending = result.catch(() => undefined);
		return result;
	}
}

registerSingleton(IRemoteDesktopService, RemoteDesktopService, InstantiationType.Delayed);
