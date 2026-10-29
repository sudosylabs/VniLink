import { Emitter, Event } from '../../src/base/common/event.js';
import { DisposableStore } from '../../src/base/common/lifecycle.js';
import { IConfigurationService } from '../../src/platform/configuration/common/configuration.js';
import { InstantiationService } from '../../src/platform/instantiation/common/instantiationService.js';
import { ServiceCollection } from '../../src/platform/instantiation/common/serviceCollection.js';
import { INativeHostService } from '../../src/platform/native/common/native.js';
import '../../src/workbench/contrib/remoteDesktop/remoteDesktop.contribution.js';
import { ViewScreenAction } from '../../src/workbench/contrib/remoteDesktop/remoteDesktopActions.js';
import { RemoteDesktopView } from '../../src/workbench/contrib/remoteDesktop/remoteDesktopView.js';
import { createWebCodecsDecoder } from '../../src/workbench/services/camera/webCodecsDecoder.js';
import { IDeviceService, type DeviceSummary } from '../../src/workbench/services/devices/deviceService.js';
import { EngineDesktopSession } from '../../src/workbench/services/remoteDesktop/engineDesktopSession.js';
import { IRemoteDesktopService } from '../../src/workbench/services/remoteDesktop/remoteDesktopService.js';
import { ITabService } from '../../src/workbench/services/tabs/tabService.js';
import { TabService } from '../../src/workbench/services/tabs/tabServiceImpl.js';
import still from './fixtures/still.json' with { type: 'json' };
import { DesktopViewFakes } from './remoteDesktopServices.js';

function assert(condition: boolean, message: string): asserts condition {
	if (!condition) {
		throw new Error(message);
	}
}

async function until(condition: () => boolean, message: string, timeout = 5000): Promise<void> {
	const deadline = performance.now() + timeout;
	while (!condition()) {
		if (performance.now() > deadline) {
			throw new Error(message);
		}
		await new Promise(resolve => setTimeout(resolve, 10));
	}
}

function bytes(base64: string): Uint8Array {
	return Uint8Array.from(atob(base64), character => character.charCodeAt(0));
}

/** PSNR of two RGBA pictures over their colour channels. */
function psnr(left: Uint8Array | Uint8ClampedArray, right: Uint8Array | Uint8ClampedArray): number {
	let sum = 0;
	let count = 0;
	for (let at = 0; at < left.length; at += 4) {
		for (let channel = 0; channel < 3; channel++) {
			const error = left[at + channel] - right[at + channel];
			sum += error * error;
			count++;
		}
	}
	return 10 * Math.log10(255 * 255 / Math.max(sum / count, 1e-9));
}

/**
 * A still picture as the host's `StillEncoder` makes it (AV1 4:4:4, in
 * `fixtures/still.json`, written by the `write_still_fixture` Rust test), shown as the app shows one: the
 * engine session's own stills decoder through WebCodecs, then the remote
 * desktop view's canvas. A coloured glyph on the canvas must keep its
 * colour far better than 4:2:0 keeps it.
 */
export async function verifyRemoteDesktopStill(): Promise<void> {
	const disposables = new DisposableStore();
	const device: DeviceSummary = { id: 'studio', name: 'Studio PC', os: 'windows', presence: 'connected', hostname: 'studio-pc', direct: 1, relays: 0 };
	const sessions: EngineDesktopSession[] = [];
	const tabService = disposables.add(new TabService());
	const services = disposables.add(new InstantiationService(new ServiceCollection(
		...new DesktopViewFakes().services(disposables),
		[ITabService, tabService],
		[IDeviceService, { _serviceBrand: undefined, devices: [device] }],
		[INativeHostService, { _serviceBrand: undefined, onDidChangeWindowState: Event.None, setFullScreen: () => Promise.resolve() }],
		[IConfigurationService, { _serviceBrand: undefined, getValue: () => undefined, onDidChangeConfiguration: Event.None }],
		[IRemoteDesktopService, {
			_serviceBrand: undefined,
			open: (_deviceId: string, control: boolean) => {
				const session = new EngineDesktopSession(control, { id: '4', generation: '2' }, createWebCodecsDecoder, () => Promise.resolve([]));
				const video = {
					onCursor: disposables.add(new Emitter<never>()).event,
					requestKeyframe: () => undefined,
					mediaStatus: () => Promise.resolve(undefined),
					desktopAudio: () => Promise.reject(new Error('No sound in this test')),
					close: () => undefined,
				};
				session.attach({ video, input: undefined });
				sessions.push(session);
				return Promise.resolve({ session });
			},
		}],
	), true));
	const host = document.createElement('div');
	host.style.cssText = 'position:fixed;left:0;top:40px;width:1280px;height:720px;z-index:99;display:flex;';
	document.body.append(host);
	try {
		services.invokeFunction(accessor => {
			new ViewScreenAction().run(accessor, device.id);
		});
		const view = tabService.activeTab;
		assert(view instanceof RemoteDesktopView, 'View Screen did not open a tab');
		host.append(view.element);
		const canvas = view.element.querySelector<HTMLCanvasElement>('.desktop-screen');
		assert(canvas !== null, 'The tab has no screen');
		await until(() => sessions.length === 1, 'The tab did not open the screen');
		const metadata = { codec: 'av1', keyframe: true, timestampUs: '1', displayId: '7', revision: '3', hostUs: 0, queuedUs: 0, role: 'still' };
		await sessions[0].consume({ type: 'data', sequence: 1, kind: 'video', data: bytes(still.still), metadata: JSON.stringify(metadata) });
		await until(() => canvas.width === still.width && canvas.height === still.height, `The still was not drawn: the canvas is ${String(canvas.width)} × ${String(canvas.height)}`);
		const { x, y, width, height } = still.crop;
		const drawn = canvas.getContext('2d')?.getImageData(x, y, width, height).data;
		assert(drawn !== undefined, 'The screen cannot be read back');
		const kept = psnr(bytes(still.glyph), drawn);
		assert(kept > still.subsampledPsnr + 6, `The coloured glyph kept ${kept.toFixed(1)} dB, against ${String(still.subsampledPsnr)} dB at 4:2:0: the still did not keep its colour`);
	} finally {
		host.remove();
		disposables.dispose();
	}
}
