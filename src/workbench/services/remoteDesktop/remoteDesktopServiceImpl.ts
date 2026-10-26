import { InstantiationType, registerSingleton } from '../../../platform/instantiation/common/extensions.js';
import { IEngineService } from '../../../platform/engine/common/engineService.js';
import type { DataChannelRequest, DataPacket } from '../../../platform/engine/common/dataChannel.js';
import { engineErrorCategory, type DesktopInputCommand, type RemoteStatus, type VideoCodec, type VideoCodecCaps, type WindowDecoder } from '../../../platform/engine/common/engine.js';
import { DataChannelClient } from '../../../platform/engine/electron-browser/dataChannelClient.js';
import { createWebCodecsDecoder, webCodecsSupport } from '../camera/webCodecsDecoder.js';
import { activeControllerSession } from '../devices/controllerSession.js';
import { IDeviceService } from '../devices/deviceService.js';
import { EngineDesktopSession, type DesktopVideoChannel } from './engineDesktopSession.js';
import { probeWindowDecoders, type DecoderSupport } from './windowDecoders.js';
import { type DesktopFailure, type IDesktopSession, IRemoteDesktopService } from './remoteDesktopService.js';

export type DesktopChannelOpener = (
	request: Omit<DataChannelRequest, 'requestId'>,
	consume: (packet: DataPacket) => Promise<void>,
	onError: (error: Error) => void,
) => Promise<DesktopVideoChannel & { input(command: DesktopInputCommand): void }>;

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
	protected openChannel: DesktopChannelOpener;
	protected decoderSupport: DecoderSupport = webCodecsSupport;
	/** What this window decodes, asked once. */
	private decoders: Promise<WindowDecoder[]> | undefined;
	/** Codecs WebCodecs said it decodes, then couldn't show: offered no more. */
	private readonly unshown = new Set<VideoCodec>();

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
		await this.registerDecoders();
		const session = new EngineDesktopSession(control, target, createWebCodecsDecoder, async () => {
			const reply = await this.engine.execute({ type: 'sessionTopology', sessionId: target.id, generation: target.generation });
			return reply.type === 'displays' ? reply.displays : [];
		});
		// A codec this window could not show is left out of the next offer.
		const unshown = session.onEnd(end => {
			unshown.dispose();
			const codec = session.shownCodec;
			if (end === 'cantShow' && codec) {
				this.unshown.add(codec);
			}
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

	/**
	 * Tells the engine which codecs this window decodes, so the other
	 * computer sends one it can show, in hardware when it can. Without an
	 * answer the engine offers what its own decoders take.
	 */
	private async registerDecoders(): Promise<void> {
		this.decoders ??= probeWindowDecoders(this.decoderSupport);
		try {
			const probed = await this.decoders;
			// A window whose WebCodecs answered nothing says nothing either.
			if (!probed.length) {
				return;
			}
			const decoders = probed.filter(decoder => !this.unshown.has(decoder.codec));
			await this.engine.execute({ type: 'setWindowDecoders', decoders });
		} catch {
			// The offer falls back to the engine's own decoders.
		}
	}

	public get available(): boolean {
		return this.engine.available;
	}

	public async codecs(): Promise<Readonly<VideoCodecCaps>> {
		if (!this.available) {
			throw new Error('Desktop engine unavailable');
		}
		const reply = await this.engine.execute({ type: 'videoCodecs' });
		if (reply.type !== 'videoCodecs') {
			throw new Error('Invalid video codecs reply');
		}
		return Object.freeze({ ...reply.codecs });
	}
}

registerSingleton(IRemoteDesktopService, RemoteDesktopService, InstantiationType.Delayed);
