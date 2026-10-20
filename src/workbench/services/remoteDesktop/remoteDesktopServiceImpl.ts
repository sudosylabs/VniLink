import { InstantiationType, registerSingleton } from '../../../platform/instantiation/common/extensions.js';
import { IEngineService } from '../../../platform/engine/common/engineService.js';
import type { DataChannelRequest, DataPacket } from '../../../platform/engine/common/dataChannel.js';
import { deviceName, engineErrorCategory, type DisplayInfo, type EngineCommand, type QualityLimits, type RemoteStatus } from '../../../platform/engine/common/engine.js';
import { DataChannelClient } from '../../../platform/engine/electron-browser/dataChannelClient.js';
import { createWebCodecsDecoder } from '../camera/webCodecsDecoder.js';
import { activeControllerSession } from '../devices/controllerSession.js';
import { IDeviceService } from '../devices/deviceService.js';
import { EngineDesktopSession, type DesktopVideoChannel } from './engineDesktopSession.js';
import { DesktopReadbackError, desktopTargetKey, isValidDesktopQuality, type DesktopFailure, type DesktopSession, type DesktopSettings, type DesktopTarget, type IDesktopSession, IRemoteDesktopService } from './remoteDesktopService.js';

export type DesktopChannelOpener = (
	request: Omit<DataChannelRequest, 'requestId'>,
	consume: (packet: DataPacket) => Promise<void>,
	onError: (error: Error) => void,
) => Promise<DesktopVideoChannel & { input(command: EngineCommand): void }>;

/** What the other computer reports about its screen, as a reason it can't be opened. */
export function desktopRefusal(status: RemoteStatus | undefined): DesktopFailure | undefined {
	switch (status) {
		case 'available': return undefined;
		case 'turnedOff': return 'turnedOff';
		case 'needsPermission': return 'needsPermission';
		case 'unavailable': return 'unavailable';
		case 'unsupported':
		case undefined: return 'unsupported';
	}
}

/** Why the other computer refused to open its screen, from the engine's error category. */
export function desktopOpenFailure(error: unknown): DesktopFailure {
	switch (engineErrorCategory(error) ?? '') {
		case 'Unauthorized':
		case 'TransferDenied': return 'turnedOff';
		case 'Busy': return 'unavailable';
		case 'NoSession':
		case 'StaleSession': return 'disconnected';
		default: return 'failed';
	}
}

export class RemoteDesktopService implements IRemoteDesktopService {
	declare public readonly _serviceBrand: undefined;
	private pending: Promise<unknown> = Promise.resolve();
	protected openChannel: DesktopChannelOpener;

	constructor(
		@IEngineService private readonly engine: IEngineService,
		@IDeviceService private readonly devices: IDeviceService,
	) {
		this.openChannel = (request, consume, onError) => DataChannelClient.open(engine, request, consume, onError);
	}

	public async open(deviceId: string, control: boolean): Promise<{ readonly failure: DesktopFailure } | { readonly session: IDesktopSession }> {
		let report;
		try {
			report = await this.devices.availability(deviceId);
		} catch {
			return { failure: 'failed' };
		}
		if (!report) {
			return { failure: 'disconnected' };
		}
		const refused = desktopRefusal(report.find(entry => entry.capability === (control ? 'desktopControl' : 'desktopView'))?.status);
		if (refused) {
			return { failure: refused };
		}
		const target = await activeControllerSession(this.engine, deviceId);
		if (!target) {
			return { failure: 'disconnected' };
		}
		const session = new EngineDesktopSession(control, target, createWebCodecsDecoder, async () => {
			const reply = await this.engine.execute({ type: 'sessionTopology', sessionId: target.id, generation: target.generation });
			return reply.type === 'displays' ? reply.displays : [];
		});
		const request = { sessionId: target.id, generation: target.generation };
		const lost = (): void => {
			session.lost();
		};
		let video: DesktopVideoChannel | undefined;
		try {
			video = await this.openChannel({ kind: 'video', ...request }, packet => session.consume(packet), lost);
			// Input shares the screen's owner, so it opens once the picture is admitted.
			const input = control ? await this.openChannel({ kind: 'input', ...request }, () => Promise.resolve(), lost) : undefined;
			session.attach({ video, input });
		} catch (error) {
			video?.close();
			session.dispose();
			return { failure: desktopOpenFailure(error) };
		}
		return { session };
	}

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
			const name = peer ? deviceName(peer) : session.peer_id;
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
