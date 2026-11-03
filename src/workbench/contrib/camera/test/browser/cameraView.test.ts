// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'assert';
import '../../../../media/workbench.css';
import '../../camera.contribution.js';
import { $ } from '../../../../../base/browser/dom.js';
import { timeout } from '../../../../../base/common/async.js';
import type { VSBuffer } from '../../../../../base/common/buffer.js';
import { Emitter, Event as BaseEvent } from '../../../../../base/common/event.js';
import { Disposable } from '../../../../../base/common/lifecycle.js';
import { until } from '../../../../../base/test/browser/waits.js';
import { ensureNoDisposablesAreLeakedInTestSuite } from '../../../../../base/test/common/utils.js';
import { runWithFakedTimers } from '../../../../../base/test/common/virtualScheduling/index.js';
import type { RecordingStatus } from '../../../../../platform/engine/common/engine.js';
import { TestInstantiationService } from '../../../../../platform/instantiation/test/common/instantiationServiceMock.js';
import { INativeHostService, type IWindowState } from '../../../../../platform/native/common/native.js';
import { IRecordingService } from '../../../../../platform/recording/common/recording.js';
import { ICameraService, type CameraEnd, type CameraFailure, type CameraList, type ICameraStream } from '../../../../services/camera/cameraService.js';
import { EngineCameraStream } from '../../../../services/camera/engineCameraStream.js';
import { createWebCodecsDecoder } from '../../../../services/camera/webCodecsDecoder.js';
import { IDeviceService, type DeviceSummary } from '../../../../services/devices/deviceService.js';
import { TestDeviceService } from '../../../../services/devices/test/common/testDeviceService.js';
import { INativeRuntimeService } from '../../../../services/nativeRuntime/nativeRuntimeService.js';
import { ITabService } from '../../../../services/tabs/tabService.js';
import { TabService } from '../../../../services/tabs/tabServiceImpl.js';
import { OpenCameraAction } from '../../cameraActions.js';
import { CameraView } from '../../cameraView.js';

/** Presses `element` as a pointer does. */
function click(element: Element | null | undefined): void {
	element?.dispatchEvent(new MouseEvent('click', { bubbles: true, cancelable: true }));
}

/** A camera's picture the test plays and ends. */
class TestCameraStream implements ICameraStream {
	private readonly frames = new Emitter<VideoFrame>();
	public readonly onFrame = this.frames.event;
	private readonly ends = new Emitter<CameraEnd>();
	public readonly onEnd = this.ends.event;
	public disposed = false;

	constructor(public readonly cameraId: string) { }

	/** Shows one plain picture of `width` × `height`. */
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

const twoCameras: CameraList = { cameras: [{ id: 'front', name: 'FaceTime Camera' }, { id: 'desk', name: 'Desk Camera' }], recording: true };

const oneCamera: CameraList = { cameras: [{ id: 'desk', name: 'Desk Camera' }], recording: false };

const recordingStatus = { id: '1', phase: 'Recording', failure: null, frames: '0', bytes: '0', durationMs: '0', segments: 0 } satisfies RecordingStatus;

/** Encodes a few pictures as Annex-B H.264, as the host's encoders deliver them: a keyframe first. None where this window has no encoder. */
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
 * The camera tab over a scripted camera service, mounted at 640×360:
 * `listings` and `openings` answer the next camera list and open, and what
 * the tab saves, copies, records and asks of the window is kept.
 */
class CameraFixture extends Disposable {
	public readonly listings: ({ readonly failure: CameraFailure } | CameraList)[] = [];
	public readonly openings: (CameraFailure | undefined)[] = [];
	public readonly streams: TestCameraStream[] = [];
	public readonly recordings: string[] = [];
	public recording: RecordingStatus = recordingStatus;
	public readonly released: string[] = [];
	public readonly saved: { name: string; png: VSBuffer }[] = [];
	private readonly didSave = this._register(new Emitter<void>());
	/** Fires once a picture is saved. */
	public readonly onDidSave = this.didSave.event;
	public readonly copied: VSBuffer[] = [];
	private readonly didCopy = this._register(new Emitter<void>());
	/** Fires once a picture is on the clipboard. */
	public readonly onDidCopy = this.didCopy.event;
	public readonly fullScreen: boolean[] = [];
	public readonly tabService = this._register(new TabService());
	private readonly windowState = this._register(new Emitter<IWindowState>());
	private readonly instantiationService: TestInstantiationService;
	private readonly host = $('div');

	constructor() {
		super();
		this.instantiationService = this._register(new TestInstantiationService());
		this.instantiationService.stub(ITabService, this.tabService);
		const devices = this._register(new TestDeviceService());
		devices.devices = [device];
		this.instantiationService.stub(IDeviceService, devices);
		this.instantiationService.stub(IRecordingService, {
			savePicture: (name: string, png: VSBuffer) => {
				this.saved.push({ name, png });
				this.didSave.fire();
				return Promise.resolve();
			},
		});
		this.instantiationService.stub(INativeRuntimeService, {
			recordingStatus: () => Promise.resolve(this.recording),
			stopRecording: () => {
				this.recording = { ...this.recording, phase: 'Finished' };
				return Promise.resolve(this.recording);
			},
			releaseRecording: (id: string) => {
				this.released.push(id);
				return Promise.resolve();
			},
		});
		this.instantiationService.stub(INativeHostService, {
			onDidChangeWindowState: this.windowState.event,
			setFullScreen: (value: boolean) => {
				this.fullScreen.push(value);
				return Promise.resolve();
			},
			writeClipboardPicture: (png: VSBuffer) => {
				this.copied.push(png);
				this.didCopy.fire();
				return Promise.resolve();
			},
		});
		this.instantiationService.stub(ICameraService, {
			cameras: () => Promise.resolve(this.listings.shift() ?? { failure: 'failed' }),
			open: (_deviceId: string, cameraId: string) => {
				const failure = this.openings.shift();
				if (failure) {
					return Promise.resolve({ failure });
				}
				const stream = new TestCameraStream(cameraId);
				this.streams.push(stream);
				return Promise.resolve({ stream });
			},
			startRecording: (deviceId: string) => {
				this.recordings.push(deviceId);
				return Promise.resolve({ recording: '1' });
			},
		});
		this.host.className = 'view-host';
		this.host.style.cssText = 'position:fixed;left:240px;top:80px;width:640px;height:360px;z-index:99;display:flex;';
		document.body.append(this.host);
	}

	/** Runs the Camera row's action for the computer and returns the tab it shows, unmounted. */
	public run(): CameraView {
		this.instantiationService.invokeFunction(accessor => {
			new OpenCameraAction().run(accessor, device.id);
		});
		const view = this.tabService.activeTab;
		assert.ok(view instanceof CameraView, 'The Camera row did not open a camera tab');
		return view;
	}

	/** Opens the camera tab over `listing` and mounts it. */
	public open(listing: { readonly failure: CameraFailure } | CameraList = twoCameras): CameraView {
		this.listings.push(listing);
		const view = this.run();
		this.host.append(view.element);
		return view;
	}

	/** Opens the camera tab over `listing` and plays a 640×360 picture in it. */
	public async play(listing: CameraList = twoCameras): Promise<CameraView> {
		const view = this.open(listing);
		await until(() => this.streams.length === 1, 'The first camera did not open');
		this.streams[0].frame(640, 360);
		await until(() => !control(view, 'camera-shutter').disabled, 'The picture did not play');
		return view;
	}

	public override dispose(): void {
		for (const tab of [...this.tabService.tabs]) {
			tab.dispose();
		}
		super.dispose();
		this.host.remove();
	}
}

function message(view: CameraView): string {
	return view.element.querySelector('.camera-message:not([hidden]) p')?.textContent ?? '';
}

function retry(view: CameraView): HTMLElement | null {
	return view.element.querySelector<HTMLElement>('.camera-message .monaco-button:not([hidden])');
}

function control(view: CameraView, name: string): HTMLButtonElement {
	const button = view.element.querySelector<HTMLButtonElement>(`.camera-controls .${name}`);
	assert.ok(button, `The ${name} control is missing`);
	return button;
}

function notice(view: CameraView): string {
	return view.element.querySelector('.camera-notice')?.textContent ?? '';
}

suite('CameraView', () => {
	const disposables = ensureNoDisposablesAreLeakedInTestSuite();
	let fixture: CameraFixture;
	setup(() => {
		fixture = disposables.add(new CameraFixture());
	});
	test('the Camera row opens a tab, a region named for the computer', () => {
		const view = fixture.open();
		assert.strictEqual(view.id, `camera:${device.id}`);
		assert.strictEqual(view.label, 'Studio Mac Camera');
		assert.strictEqual(view.element.getAttribute('role'), 'region');
		assert.strictEqual(view.element.getAttribute('aria-label'), 'Camera on Studio Mac');
	});
	test('the tab says it is connecting until the camera opens', () => {
		const view = fixture.open();
		assert.strictEqual(message(view), 'Connecting to the camera on Studio Mac…');
	});
	test('opening the camera again keeps one tab', () => {
		const view = fixture.open();
		fixture.run();
		assert.strictEqual(fixture.tabService.tabs.filter(tab => tab.id === view.id).length, 1);
	});
	test('two cameras show a named picker with the first chosen', async () => {
		const view = fixture.open();
		await until(() => fixture.streams.length === 1, 'The first camera did not open');
		const picker = view.element.querySelector('select');
		assert.strictEqual(fixture.streams[0].cameraId, 'front');
		assert.ok(picker && !picker.hidden);
		assert.strictEqual(picker.getAttribute('aria-label'), 'Camera');
	});
	test('the controls are a toolbar over the picture', async () => {
		const view = fixture.open();
		await until(() => fixture.streams.length === 1, 'The first camera did not open');
		assert.strictEqual(view.element.querySelector('.camera-controls')?.getAttribute('role'), 'toolbar');
	});
	test('nothing can be taken before a picture plays', async () => {
		const view = fixture.open();
		await until(() => fixture.streams.length === 1, 'The first camera did not open');
		assert.strictEqual(control(view, 'camera-shutter').disabled, true);
		assert.strictEqual(control(view, 'camera-record').disabled, true);
	});
	test('a playing picture draws at its size, hides the message and can be taken', async () => {
		const view = fixture.open();
		await until(() => fixture.streams.length === 1, 'The first camera did not open');
		fixture.streams[0].frame(640, 360);
		const canvas = view.element.querySelector('canvas');
		await until(() => canvas?.width === 640 && canvas.height === 360, 'The picture did not draw at its size');
		assert.strictEqual(message(view), '');
		assert.strictEqual(control(view, 'camera-shutter').disabled, false);
	});
	test('the picture fills the tab, and Fit shows the whole picture and back', async () => {
		const view = await fixture.play();
		const canvas = view.element.querySelector('canvas');
		assert.ok(canvas);
		assert.strictEqual(getComputedStyle(canvas).objectFit, 'cover');
		assert.strictEqual(canvas.getBoundingClientRect().width, 640);
		const fit = control(view, 'camera-fit');
		assert.strictEqual(fit.getAttribute('aria-label'), 'Show the Whole Picture');
		click(fit);
		assert.strictEqual(getComputedStyle(canvas).objectFit, 'contain');
		assert.strictEqual(fit.getAttribute('aria-label'), 'Fill the Tab');
		click(fit);
		assert.strictEqual(getComputedStyle(canvas).objectFit, 'cover');
	});
	test('the shutter saves a PNG named for the computer and the time, and says so', async () => {
		const view = await fixture.play();
		const saved = BaseEvent.toPromise(fixture.onDidSave);
		click(control(view, 'camera-shutter'));
		await saved;
		assert.strictEqual(fixture.saved.length, 1);
		assert.match(fixture.saved[0].name, /^Studio Mac \d{4}-\d{2}-\d{2} \d{2}\.\d{2}\.\d{2}\.png$/);
		assert.deepStrictEqual([...fixture.saved[0].png.buffer.slice(1, 4)], [0x50, 0x4e, 0x47]);
		await until(() => notice(view) === 'Picture saved to the recordings folder.', 'Taking a picture was not confirmed');
	});
	test('copying places the picture on the clipboard', async () => {
		await fixture.play();
		const copied = BaseEvent.toPromise(fixture.onDidCopy);
		// Cmd/Ctrl+C reaches the page as a copy event.
		document.body.dispatchEvent(new ClipboardEvent('copy', { bubbles: true, cancelable: true }));
		await copied;
		assert.strictEqual(fixture.copied.length, 1);
	});
	test('Record is offered where the computer allows it', async () => {
		const view = await fixture.play();
		const record = control(view, 'camera-record');
		assert.strictEqual(record.disabled, false);
		assert.strictEqual(record.getAttribute('aria-label'), 'Record');
		assert.strictEqual(record.getAttribute('aria-pressed'), 'false');
	});
	suite('recording', () => {
		/** Plays the camera and starts recording it; time is virtual where this is called. */
		async function startRecording(): Promise<{ readonly view: CameraView; readonly record: HTMLButtonElement; readonly badge: HTMLElement }> {
			const view = await fixture.play();
			const record = control(view, 'camera-record');
			click(record);
			await timeout(0);
			assert.deepStrictEqual(fixture.recordings, [device.id]);
			assert.strictEqual(record.getAttribute('aria-pressed'), 'true');
			const badge = view.element.querySelector<HTMLElement>('.camera-recording');
			assert.ok(badge);
			return { view, record, badge };
		}
		test('Record starts a recording, shown running with Stop Recording', () => runWithFakedTimers({}, async () => {
			const { record, badge } = await startRecording();
			assert.strictEqual(badge.hidden, false);
			assert.strictEqual(record.getAttribute('aria-label'), 'Stop Recording');
			click(record);
			await timeout(0);
		}));
		test('the recording time follows the recording', () => runWithFakedTimers({}, async () => {
			const { record, badge } = await startRecording();
			fixture.recording = { ...recordingStatus, durationMs: '65000' };
			await timeout(1000);
			assert.strictEqual(badge.textContent, '1:05');
			click(record);
			await timeout(0);
		}));
		test('stopping finishes and releases the recording, and says it was saved', () => runWithFakedTimers({}, async () => {
			const { view, record, badge } = await startRecording();
			click(record);
			await timeout(0);
			assert.strictEqual(badge.hidden, true);
			assert.deepStrictEqual(fixture.released, ['1']);
			assert.strictEqual(notice(view), 'Recording saved to the recordings folder.');
		}));
	});
	test('an untouched playing picture hides the controls, and moving the pointer shows them', () => runWithFakedTimers({}, async () => {
		const view = await fixture.play();
		await timeout(3000);
		assert.strictEqual(view.element.classList.contains('camera-idle'), true);
		view.element.dispatchEvent(new MouseEvent('mousemove', { bubbles: true }));
		assert.strictEqual(view.element.classList.contains('camera-idle'), false);
	}));
	test('full screen covers the window, and Escape leaves it', async () => {
		const view = await fixture.play();
		click(control(view, 'camera-fullscreen'));
		await until(() => fixture.fullScreen.join() === 'true' && view.element.classList.contains('camera-view-fullscreen'), 'Full screen did not cover the window');
		assert.strictEqual(control(view, 'camera-fullscreen').getAttribute('aria-label'), 'Exit Full Screen');
		document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true, cancelable: true }));
		await until(() => fixture.fullScreen.join() === 'true,false' && !view.element.classList.contains('camera-view-fullscreen'), 'Escape did not leave full screen');
	});
	test('choosing another camera replaces the stream', async () => {
		const view = await fixture.play();
		const picker = view.element.querySelector('select');
		assert.ok(picker);
		picker.value = 'desk';
		picker.dispatchEvent(new Event('change'));
		await until(() => fixture.streams.length === 2 && fixture.streams[1].cameraId === 'desk' && fixture.streams[0].disposed, 'Switching cameras did not replace the stream');
	});
	for (const [end, text] of [['withdrawn', 'Studio Mac stopped sharing its camera.'], ['stopped', 'The camera on Studio Mac stopped.'], ['lost', 'Lost the connection to Studio Mac.']] as const) {
		test(`a camera that ends as ${end} says "${text}", announced, keeps the picture dimmed, and Try Again reopens it`, async () => {
			const view = await fixture.play();
			fixture.listings.push(oneCamera);
			fixture.streams[0].end(end);
			await until(() => message(view) === text, `Missing: ${text}`);
			assert.strictEqual(view.element.querySelector('.camera-message p')?.getAttribute('role'), 'alert');
			assert.notStrictEqual(view.element.querySelector('.camera-stage-dimmed'), null);
			click(retry(view));
			await until(() => fixture.streams.length === 2, `Try Again did not reopen after ${end}`);
		});
	}
	test('one camera hides the picker, and recording is not offered where it is not allowed', async () => {
		const view = await fixture.play(oneCamera);
		assert.strictEqual(view.element.querySelector<HTMLElement>('.camera-picker')?.hidden, true);
		assert.strictEqual(control(view, 'camera-record').disabled, true);
		assert.strictEqual(control(view, 'camera-record').getAttribute('aria-label'), 'Studio Mac doesn\'t allow recording.');
	});
	for (const [failure, text, again] of [
		['inUse', 'Another computer is watching this camera.', true],
		['unavailable', 'Studio Mac can\'t open another camera right now.', true],
		['turnedOff', 'Studio Mac doesn\'t share its camera with this computer.', false],
		['needsPermission', 'Studio Mac hasn\'t allowed VniLink to use its camera.', false],
		['noCamera', 'Studio Mac has no camera connected.', false],
		['disconnected', 'Studio Mac isn\'t connected.', false],
	] as const) {
		test(`a camera that cannot open as ${failure} says "${text}"${again ? ' and offers' : ' without'} Try Again, with nothing dimmed`, async () => {
			const closed = await fixture.play(oneCamera);
			await fixture.tabService.close(closed.id);
			if (failure === 'inUse') {
				fixture.openings.push('inUse');
			}
			const view = fixture.open(failure === 'inUse' ? oneCamera : { failure });
			assert.notStrictEqual(view, closed, 'a closed camera tab was reused');
			await until(() => message(view) === text, `Missing: ${text} (shown: ${message(view)})`);
			assert.strictEqual(retry(view) !== null, again);
			assert.strictEqual(view.element.querySelector('.camera-stage-dimmed'), null);
		});
	}
	test('the host\'s H.264 pictures decode through WebCodecs at their size', async function () {
		const encoded = await encodedPictures();
		if (!encoded.length) {
			// This window cannot encode H.264, so there is nothing to decode.
			this.skip();
		}
		const decoded = disposables.add(new EngineCameraStream(createWebCodecsDecoder));
		const sizes: string[] = [];
		let ended: CameraEnd | undefined;
		disposables.add(decoded.onFrame(frame => {
			sizes.push(`${String(frame.displayWidth)}×${String(frame.displayHeight)}`);
			frame.close();
		}));
		disposables.add(decoded.onEnd(end => {
			ended = end;
		}));
		let sequence = 0;
		for (const { data, key } of encoded) {
			await decoded.consume({ type: 'data', sequence: ++sequence, kind: 'camera', data, metadata: JSON.stringify({ codec: 'h264', keyframe: key, timestampUs: String(sequence * 33_333), width: 320, height: 240 }) });
		}
		await until(() => sizes.length > 0 || ended !== undefined, 'WebCodecs neither decoded the host\'s H.264 pictures nor failed');
		assert.strictEqual(ended, undefined);
		assert.ok(sizes.every(size => size === '320×240'), sizes.join());
	});
});
