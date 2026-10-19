import { Emitter } from '../../src/base/common/event.js';
import { DisposableStore } from '../../src/base/common/lifecycle.js';
import type { VSBuffer } from '../../src/base/common/buffer.js';
import type { RecordingStatus } from '../../src/platform/engine/common/engine.js';
import { InstantiationService } from '../../src/platform/instantiation/common/instantiationService.js';
import { ServiceCollection } from '../../src/platform/instantiation/common/serviceCollection.js';
import { INativeHostService, type IWindowState } from '../../src/platform/native/common/native.js';
import { IRecordingService } from '../../src/platform/recording/common/recording.js';
import { OpenCameraAction } from '../../src/workbench/contrib/camera/cameraActions.js';
import { CameraView } from '../../src/workbench/contrib/camera/cameraView.js';
import '../../src/workbench/contrib/camera/camera.contribution.js';
import { ICameraService, type CameraEnd, type CameraFailure, type CameraList, type ICameraStream } from '../../src/workbench/services/camera/cameraService.js';
import { EngineCameraStream } from '../../src/workbench/services/camera/engineCameraStream.js';
import { createWebCodecsDecoder } from '../../src/workbench/services/camera/webCodecsDecoder.js';
import { IDeviceService, type DeviceSummary } from '../../src/workbench/services/devices/deviceService.js';
import { INativeRuntimeService } from '../../src/workbench/services/nativeRuntime/nativeRuntimeService.js';
import { ITabService } from '../../src/workbench/services/tabs/tabService.js';
import { TabService } from '../../src/workbench/services/tabs/tabServiceImpl.js';

function assert(condition: boolean, message: string): asserts condition {
	if (!condition) {
		throw new Error(message);
	}
}

async function until(condition: () => boolean, message: string, timeout = 3000): Promise<void> {
	const deadline = Date.now() + timeout;
	while (!condition()) {
		assert(Date.now() < deadline, message);
		await new Promise(resolve => setTimeout(resolve, 20));
	}
}

class FakeStream implements ICameraStream {
	private readonly frames = new Emitter<VideoFrame>();
	public readonly onFrame = this.frames.event;
	private readonly ends = new Emitter<CameraEnd>();
	public readonly onEnd = this.ends.event;
	public disposed = false;

	constructor(public readonly cameraId: string) { }

	public frame(width: number, height: number): void {
		const canvas = new OffscreenCanvas(width, height);
		const context = canvas.getContext('2d');
		if (context) {
			context.fillStyle = '#3a7';
			context.fillRect(0, 0, width, height);
		}
		this.frames.fire(new VideoFrame(canvas, { timestamp: 0 }));
	}

	public end(reason: CameraEnd): void {
		this.ends.fire(reason);
	}

	public dispose(): void {
		this.disposed = true;
		this.frames.dispose();
		this.ends.dispose();
	}
}

const device: DeviceSummary = { id: 'c'.repeat(64), name: 'Studio Mac', os: 'macos', presence: 'connected', hostname: 'studio.local', direct: 1, relays: 0 };

/** Encodes a few pictures as Annex-B H.264, as the host's encoders deliver them: a keyframe first. */
async function encodedPictures(): Promise<{ readonly data: Uint8Array; readonly key: boolean }[]> {
	const config: VideoEncoderConfig = { codec: 'avc1.42E01F', width: 320, height: 240, avc: { format: 'annexb' }, latencyMode: 'realtime' };
	if (!(await VideoEncoder.isConfigSupported(config)).supported) {
		return [];
	}
	const chunks: { data: Uint8Array; key: boolean }[] = [];
	const encoder = new VideoEncoder({
		output: chunk => {
			const data = new Uint8Array(chunk.byteLength);
			chunk.copyTo(data);
			chunks.push({ data, key: chunk.type === 'key' });
		},
		error: () => { /* reported by the missing chunks */ },
	});
	encoder.configure(config);
	const canvas = new OffscreenCanvas(320, 240);
	const context = canvas.getContext('2d');
	for (let index = 0; index < 5; index++) {
		if (context) {
			context.fillStyle = `rgb(${String(index * 40)} 120 80)`;
			context.fillRect(0, 0, 320, 240);
		}
		const frame = new VideoFrame(canvas, { timestamp: index * 33_333 });
		encoder.encode(frame, { keyFrame: index === 0 });
		frame.close();
	}
	await encoder.flush();
	encoder.close();
	return chunks;
}

/**
 * The camera tab with a scripted camera service: the picture fills the tab
 * under camera-app controls, the picker appears only for several cameras, a
 * picture can be taken or copied and the camera recorded where the other
 * computer allows it, every way a camera can't open or stops is said in
 * words, and a real H.264 picture decodes through WebCodecs.
 */
export async function verifyCamera(): Promise<boolean> {
	const disposables = new DisposableStore();
	const violations: string[] = [];
	const onViolation = (event: SecurityPolicyViolationEvent): void => {
		violations.push(`${event.violatedDirective} ${event.blockedURI}`);
	};
	document.addEventListener('securitypolicyviolation', onViolation);
	const listings: ({ readonly failure: CameraFailure } | CameraList)[] = [];
	const recordings: string[] = [];
	const recordingStatus = { id: '1', phase: 'Recording', failure: null, frames: '0', bytes: '0', durationMs: '0', segments: 0 } satisfies RecordingStatus;
	let recording: RecordingStatus = recordingStatus;
	const released: string[] = [];
	const openings: (CameraFailure | undefined)[] = [];
	const streams: FakeStream[] = [];
	const saved: { name: string; png: VSBuffer }[] = [];
	const copied: VSBuffer[] = [];
	const fullScreen: boolean[] = [];
	const windowState = disposables.add(new Emitter<IWindowState>());
	const tabService = disposables.add(new TabService());
	const services = disposables.add(new InstantiationService(new ServiceCollection(
		[ITabService, tabService],
		[IDeviceService, { _serviceBrand: undefined, devices: [device] }],
		[IRecordingService, {
			_serviceBrand: undefined,
			savePicture: (name: string, png: VSBuffer) => {
				saved.push({ name, png });
				return Promise.resolve();
			},
		}],
		[INativeRuntimeService, {
			_serviceBrand: undefined,
			recordingStatus: () => Promise.resolve(recording),
			stopRecording: () => {
				recording = { ...recording, phase: 'Finished' };
				return Promise.resolve(recording);
			},
			releaseRecording: (id: string) => {
				released.push(id);
				return Promise.resolve();
			},
		}],
		[INativeHostService, {
			_serviceBrand: undefined,
			onDidChangeWindowState: windowState.event,
			setFullScreen: (value: boolean) => {
				fullScreen.push(value);
				return Promise.resolve();
			},
			writeClipboardPicture: (png: VSBuffer) => {
				copied.push(png);
				return Promise.resolve();
			},
		}],
		[ICameraService, {
			_serviceBrand: undefined,
			cameras: () => Promise.resolve(listings.shift() ?? { failure: 'failed' }),
			open: (_deviceId: string, cameraId: string) => {
				const failure = openings.shift();
				if (failure) {
					return Promise.resolve({ failure });
				}
				const stream = new FakeStream(cameraId);
				streams.push(stream);
				return Promise.resolve({ stream });
			},
			startRecording: (deviceId: string) => {
				recordings.push(deviceId);
				return Promise.resolve({ recording: '1' });
			},
		}],
	), true));
	const host = document.createElement('div');
	host.className = 'view-host';
	host.style.cssText = 'position:fixed;left:240px;top:80px;width:640px;height:360px;z-index:99;display:flex;';
	document.body.append(host);
	const message = (view: CameraView): string => view.element.querySelector('.camera-message:not([hidden]) p')?.textContent ?? '';
	const retry = (view: CameraView): HTMLElement | null => view.element.querySelector<HTMLElement>('.camera-message .monaco-button:not([hidden])');
	const control = (view: CameraView, name: string): HTMLButtonElement => {
		const button = view.element.querySelector<HTMLButtonElement>(`.camera-controls .${name}`);
		assert(button !== null, `The ${name} control is missing`);
		return button;
	};
	try {
		listings.push({ cameras: [{ id: 'front', name: 'FaceTime Camera' }, { id: 'desk', name: 'Desk Camera' }], recording: true });
		services.invokeFunction(accessor => {
			new OpenCameraAction().run(accessor, device.id);
		});
		const view = tabService.activeTab;
		assert(view instanceof CameraView && view.id === `camera:${device.id}` && view.label === 'Studio Mac Camera', 'The Camera row did not open a camera tab');
		assert(view.element.getAttribute('role') === 'region' && view.element.getAttribute('aria-label') === 'Camera on Studio Mac', 'The camera region is not named');
		host.append(view.element);
		assert(message(view) === 'Connecting to the camera on Studio Mac…', `The connecting message is missing: ${message(view)}`);
		services.invokeFunction(accessor => {
			new OpenCameraAction().run(accessor, device.id);
		});
		assert(tabService.tabs.filter(tab => tab.id === view.id).length === 1, 'Opening the camera again opened a second tab');
		await until(() => streams.length === 1, 'The first camera did not open');
		const picker = view.element.querySelector('select');
		assert(streams[0].cameraId === 'front' && picker !== null && !picker.hidden && picker.getAttribute('aria-label') === 'Camera', 'Two cameras should show a named picker with the first chosen');
		assert(view.element.querySelector('.camera-controls')?.getAttribute('role') === 'toolbar', 'The controls are not a toolbar over the picture');
		assert(control(view, 'camera-shutter').disabled && control(view, 'camera-record').disabled, 'Nothing can be taken before a picture plays');
		streams[0].frame(640, 360);
		const canvas = view.element.querySelector('canvas');
		await until(() => canvas?.width === 640 && canvas.height === 360, 'The picture did not draw at its size');
		assert(message(view) === '' && !control(view, 'camera-shutter').disabled, 'A playing picture should hide the message and allow taking it');
		// The picture fills the tab, as in a camera app; fitting shows all of it.
		assert(canvas !== null && getComputedStyle(canvas).objectFit === 'cover' && canvas.getBoundingClientRect().width === 640, 'The picture does not fill the tab');
		const fit = control(view, 'camera-fit');
		assert(fit.getAttribute('aria-label') === 'Show the Whole Picture', 'The fit control is not named');
		fit.click();
		assert(getComputedStyle(canvas).objectFit === 'contain' && fit.getAttribute('aria-label') === 'Fill the Tab', 'Fit did not show the whole picture');
		fit.click();
		control(view, 'camera-shutter').click();
		await until(() => saved.length === 1, 'The shutter did not take a picture');
		assert(/^Studio Mac \d{4}-\d{2}-\d{2} \d{2}\.\d{2}\.\d{2}\.png$/.test(saved[0].name), `The picture is misnamed: ${saved[0].name}`);
		assert(saved[0].png.buffer[1] === 0x50 && saved[0].png.buffer[2] === 0x4e && saved[0].png.buffer[3] === 0x47, 'The picture is not a PNG');
		await until(() => view.element.querySelector('.camera-notice')?.textContent === 'Picture saved to the recordings folder.', 'Taking a picture was not confirmed');
		// Cmd/Ctrl+C reaches the page as a copy event.
		document.body.dispatchEvent(new ClipboardEvent('copy', { bubbles: true, cancelable: true }));
		await until(() => copied.length === 1, 'Copy did not place the picture on the clipboard');
		const record = control(view, 'camera-record');
		assert(!record.disabled && record.getAttribute('aria-label') === 'Record' && record.getAttribute('aria-pressed') === 'false', 'Recording is not offered where it is allowed');
		record.click();
		await until(() => recordings.length === 1 && record.getAttribute('aria-pressed') === 'true', 'Record did not start a recording');
		const badge = view.element.querySelector<HTMLElement>('.camera-recording');
		assert(badge !== null && !badge.hidden && record.getAttribute('aria-label') === 'Stop Recording', 'A running recording is not shown');
		recording = { ...recordingStatus, durationMs: '65000' };
		await until(() => badge.textContent === '1:05', `The recording time is wrong: ${badge.textContent}`);
		record.click();
		await until(() => badge.hidden && released.join() === '1', 'Stopping did not finish and release the recording');
		await until(() => view.element.querySelector('.camera-notice')?.textContent === 'Recording saved to the recordings folder.', 'The saved recording was not confirmed');

		// An untouched playing picture hides its controls; moving the pointer shows them.
		await until(() => view.element.classList.contains('camera-idle'), 'The controls did not hide over a playing picture', 5000);
		view.element.dispatchEvent(new MouseEvent('mousemove', { bubbles: true }));
		assert(!view.element.classList.contains('camera-idle'), 'Moving the pointer did not show the controls');
		control(view, 'camera-fullscreen').click();
		await until(() => fullScreen.join() === 'true' && view.element.classList.contains('camera-view-fullscreen'), 'Full screen did not cover the window');
		assert(control(view, 'camera-fullscreen').getAttribute('aria-label') === 'Exit Full Screen', 'Full screen offers no way out');
		document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true, cancelable: true }));
		await until(() => fullScreen.join() === 'true,false' && !view.element.classList.contains('camera-view-fullscreen'), 'Escape did not leave full screen');
		picker.value = 'desk';
		picker.dispatchEvent(new Event('change'));
		await until(() => streams.length === 2 && streams[1].cameraId === 'desk' && streams[0].disposed, 'Switching cameras did not replace the stream');
		for (const [end, text] of [['withdrawn', 'Studio Mac stopped sharing its camera.'], ['stopped', 'The camera on Studio Mac stopped.'], ['lost', 'Lost the connection to Studio Mac.']] as const) {
			listings.push({ cameras: [{ id: 'desk', name: 'Desk Camera' }], recording: false });
			streams.at(-1)?.end(end);
			await until(() => message(view) === text, `Missing: ${text}`);
			assert(view.element.querySelector('.camera-message p')?.getAttribute('role') === 'alert' && retry(view) !== null, `${end} should be announced with Try Again`);
			assert(view.element.querySelector('.camera-stage-dimmed') !== null, 'The last picture should stay, dimmed');
			const opened = streams.length;
			retry(view)?.click();
			await until(() => streams.length === opened + 1, `Try Again did not reopen after ${end}`);
		}
		assert(view.element.querySelector<HTMLElement>('.camera-picker')?.hidden === true, 'One camera should hide the picker');
		streams.at(-1)?.frame(640, 360);
		await until(() => !control(view, 'camera-shutter').disabled, 'The reopened camera did not play');
		assert(control(view, 'camera-record').disabled && control(view, 'camera-record').getAttribute('aria-label') === 'Studio Mac doesn\'t allow recording.', 'Recording is offered where it is not allowed');
		for (const [failure, text, again] of [
			['inUse', 'Another computer is watching this camera.', true],
			['unavailable', 'Studio Mac can\'t open another camera right now.', true],
			['turnedOff', 'Studio Mac doesn\'t share its camera with this computer.', false],
			['needsPermission', 'Studio Mac hasn\'t allowed VniLink to use its camera.', false],
			['noCamera', 'Studio Mac has no camera connected.', false],
			['disconnected', 'Studio Mac isn\'t connected.', false],
		] as const) {
			await tabService.close(view.id);
			if (failure === 'inUse') {
				listings.push({ cameras: [{ id: 'desk', name: 'Desk Camera' }], recording: false });
				openings.push('inUse');
			} else {
				listings.push({ failure });
			}
			services.invokeFunction(accessor => {
				new OpenCameraAction().run(accessor, device.id);
			});
			const failed = tabService.activeTab;
			assert(failed instanceof CameraView && failed !== view, 'A closed camera tab was reused');
			host.append(failed.element);
			await until(() => message(failed) === text, `Missing: ${text} (shown: ${message(failed)})`);
			assert((retry(failed) !== null) === again, `${failure} should ${again ? '' : 'not '}offer Try Again`);
			assert(failed.element.querySelector('.camera-stage-dimmed') === null, 'A tab that never showed a picture has nothing to dim');
		}
		const encoded = await encodedPictures();
		if (encoded.length) {
			const decoded = disposables.add(new EngineCameraStream(createWebCodecsDecoder));
			let frames = 0;
			let ended: CameraEnd | undefined;
			decoded.onFrame(frame => {
				frames++;
				assert(frame.displayWidth === 320 && frame.displayHeight === 240, 'The decoded picture has the wrong size');
				frame.close();
			});
			decoded.onEnd(end => {
				ended = end;
			});
			let sequence = 0;
			for (const { data, key } of encoded) {
				await decoded.consume({ type: 'data', sequence: ++sequence, kind: 'camera', data, metadata: JSON.stringify({ codec: 'h264', keyframe: key, timestampUs: String(sequence * 33_333), width: 320, height: 240 }) });
			}
			await until(() => frames > 0 || ended !== undefined, 'WebCodecs neither decoded the host\'s H.264 pictures nor failed');
			assert(ended === undefined, `Decoding ended: ${ended ?? ''}`);
		}
		assert(violations.length === 0, `CSP violations: ${violations.join(', ')}`);
		return true;
	} finally {
		document.removeEventListener('securitypolicyviolation', onViolation);
		for (const tab of [...tabService.tabs]) {
			await tabService.close(tab.id);
		}
		host.remove();
		disposables.dispose();
	}
}
