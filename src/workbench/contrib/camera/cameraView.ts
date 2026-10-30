// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import * as nls from '../../../nls.js';
import { $, addDisposableListener, append, EventType } from '../../../base/browser/dom.js';
import { Button } from '../../../base/browser/ui/button/button.js';
import { renderIcon } from '../../../base/browser/ui/icon/icon.js';
import { VSBuffer } from '../../../base/common/buffer.js';
import { Icons, type IconId } from '../../../base/common/icons.js';
import { Disposable, MutableDisposable, toDisposable } from '../../../base/common/lifecycle.js';
import type { RecordingStatus } from '../../../platform/engine/common/engine.js';
import { INativeHostService } from '../../../platform/native/common/native.js';
import { IRecordingService } from '../../../platform/recording/common/recording.js';
import { defaultButtonStyles } from '../../../platform/theme/browser/defaultStyles.js';
import { ICameraService, type CameraChoice, type CameraEnd, type CameraFailure, type ICameraStream } from '../../services/camera/cameraService.js';
import type { DeviceSummary } from '../../services/devices/deviceService.js';
import { INativeRuntimeService } from '../../services/nativeRuntime/nativeRuntimeService.js';
import { formatElapsed, SessionRecording, type RecordingStartFailure } from '../../services/recording/sessionRecording.js';
import { ITabService, type WorkbenchTab } from '../../services/tabs/tabService.js';

/** How long the controls stay over a playing picture after the pointer stops. */
const controlsDelay = 3000;

/** How long a confirmation stays on screen. */
const noticeDelay = 2500;

/** The camera chosen last on each computer, for this run of the application. */
const lastCamera = new Map<string, string>();

type CameraProblem = CameraFailure | CameraEnd;

/** Problems another attempt may fix; the rest need a change on either computer. */
const retryable: ReadonlySet<CameraProblem> = new Set(['inUse', 'unavailable', 'failed', 'stopped', 'withdrawn', 'lost']);

function problemMessage(name: string, problem: CameraProblem): string {
	switch (problem) {
		case 'disconnected': return nls.localize('camera.disconnected', '{0} isn\'t connected.', name);
		case 'turnedOff': return nls.localize('camera.turnedOff', '{0} doesn\'t share its camera with this computer.', name);
		case 'needsPermission': return nls.localize('camera.needsPermission', '{0} hasn\'t allowed VniLink to use its camera.', name);
		case 'unsupported': return nls.localize('camera.unsupported', '{0} can\'t share a camera.', name);
		case 'unavailable': return nls.localize('camera.unavailable', '{0} can\'t open another camera right now.', name);
		case 'noCamera': return nls.localize('camera.noCamera', '{0} has no camera connected.', name);
		case 'inUse': return nls.localize('camera.inUse', 'Another computer is watching this camera.');
		case 'failed': return nls.localize('camera.failed', 'The camera on {0} didn\'t start.', name);
		case 'stopped': return nls.localize('camera.stopped', 'The camera on {0} stopped.', name);
		case 'withdrawn': return nls.localize('camera.withdrawn', '{0} stopped sharing its camera.', name);
		case 'lost': return nls.localize('camera.lost', 'Lost the connection to {0}.', name);
		case 'cantShow': return nls.localize('camera.cantShow', 'This computer can\'t show the camera picture.');
	}
}

function recordingStartMessage(name: string, failure: RecordingStartFailure): string {
	switch (failure) {
		case 'notAllowed': return nls.localize('camera.recordingNotAllowed', '{0} doesn\'t allow recording.', name);
		case 'disconnected': return nls.localize('camera.recordingDisconnected', '{0} isn\'t connected.', name);
		case 'failed': return nls.localize('camera.recordingFailed', 'The recording didn\'t start.');
	}
}

function recordingEndMessage(status: RecordingStatus): string {
	switch (status.failure) {
		case null: return nls.localize('camera.recordingSaved', 'Recording saved to the recordings folder.');
		case 'Startup': return nls.localize('camera.recordingStartup', 'The recording didn\'t start.');
		case 'Storage': return nls.localize('camera.recordingStorage', 'The recordings folder can\'t be written to.');
		case 'SlowStorage': return nls.localize('camera.recordingSlow', 'The recordings folder couldn\'t keep up, so the recording stopped.');
		case 'Limit': return nls.localize('camera.recordingLimit', 'The recording reached its size limit.');
		case 'Media':
		case 'FormatChanged': return nls.localize('camera.recordingMedia', 'The camera picture changed, so the recording stopped.');
	}
}

function timestamp(date: Date): string {
	const two = (value: number): string => String(value).padStart(2, '0');
	return `${String(date.getFullYear())}-${two(date.getMonth() + 1)}-${two(date.getDate())} ${two(date.getHours())}.${two(date.getMinutes())}.${two(date.getSeconds())}`;
}

/** A round button over the picture. */
function controlButton(parent: HTMLElement, className: string): HTMLButtonElement {
	return append(parent, $<HTMLButtonElement>(`button.camera-control.${className}`, { type: 'button' }));
}

function describe(button: HTMLButtonElement, text: string, icon?: IconId): void {
	button.setAttribute('aria-label', text);
	button.title = text;
	if (icon) {
		button.replaceChildren(renderIcon(icon));
	}
}

/**
 * A paired computer's camera, filling its tab, with its controls over the
 * picture as a camera app has them. One tab per computer, since a camera has
 * one viewer at a time. The picture is never an input surface.
 */
export class CameraView extends Disposable implements WorkbenchTab {
	public static readonly PREFIX = 'camera:';
	public readonly id: string;
	public readonly label: string;
	public readonly icon = Icons.camera;
	public readonly element: HTMLElement;
	private readonly stage: HTMLElement;
	private readonly canvas: HTMLCanvasElement;
	private readonly context: CanvasRenderingContext2D | null;
	private readonly message: HTMLElement;
	private readonly messageText: HTMLElement;
	private readonly retry: Button;
	private readonly recordingBadge: HTMLElement;
	private readonly recordingTime: HTMLElement;
	private readonly controls: HTMLElement;
	private readonly picker: HTMLSelectElement;
	private readonly shutter: HTMLButtonElement;
	private readonly record: HTMLButtonElement;
	private readonly fit: HTMLButtonElement;
	private readonly fullScreenButton: HTMLButtonElement;
	private readonly notice: HTMLElement;
	private readonly stream = this._register(new MutableDisposable<ICameraStream>());
	private readonly idleTimer = this._register(new MutableDisposable());
	private readonly noticeTimer = this._register(new MutableDisposable());
	private readonly recording: SessionRecording;
	private cameras: readonly CameraChoice[] = [];
	private recordingAllowed = false;
	private attempt = 0;
	private hasPicture = false;
	private playing = false;
	private fitted = false;
	private fullScreen = false;

	constructor(
		private readonly device: DeviceSummary,
		@ICameraService private readonly cameraService: ICameraService,
		@ITabService private readonly tabService: ITabService,
		@INativeHostService private readonly nativeHost: INativeHostService,
		@INativeRuntimeService nativeRuntime: INativeRuntimeService,
		@IRecordingService private readonly recordingService: IRecordingService,
	) {
		super();
		this.id = CameraView.idFor(device.id);
		this.label = nls.localize('camera.label', '{0} Camera', device.name);
		this.element = $('section.camera-view');
		this.element.setAttribute('role', 'region');
		this.element.tabIndex = -1;
		this.element.setAttribute('aria-label', nls.localize('camera.region', 'Camera on {0}', device.name));
		this.stage = append(this.element, $('.camera-stage'));
		this.canvas = append(this.stage, $<HTMLCanvasElement>('canvas'));
		this.canvas.setAttribute('aria-hidden', 'true');
		this.context = this.canvas.getContext('2d');
		this.message = append(this.element, $('.camera-message'));
		this.messageText = append(this.message, $('p'));
		this.retry = this._register(new Button(append(this.message, $('div')), { ...defaultButtonStyles, secondary: true, title: false }));
		this.retry.label = nls.localize('camera.retry', 'Try Again');
		this.recordingBadge = append(this.element, $('.camera-recording'));
		append(this.recordingBadge, $('span.camera-recording-dot', { 'aria-hidden': 'true' }));
		this.recordingTime = append(this.recordingBadge, $('span'));
		this.recordingBadge.hidden = true;
		this.recording = this._register(new SessionRecording(() => this.cameraService.startRecording(this.device.id), nativeRuntime));
		this.controls = append(this.element, $('.camera-controls', { role: 'toolbar', 'aria-label': nls.localize('camera.controls', 'Camera controls') }));
		const start = append(this.controls, $('.camera-controls-side'));
		this.picker = append(start, $<HTMLSelectElement>('select.camera-picker'));
		this.picker.setAttribute('aria-label', nls.localize('camera.choose', 'Camera'));
		this.picker.hidden = true;
		const middle = append(this.controls, $('.camera-controls-middle'));
		this.shutter = controlButton(middle, 'camera-shutter');
		describe(this.shutter, nls.localize('camera.takePicture', 'Take Picture'));
		this.record = controlButton(middle, 'camera-record');
		const end = append(this.controls, $('.camera-controls-side.camera-controls-end'));
		this.fit = controlButton(end, 'camera-fit');
		this.fullScreenButton = controlButton(end, 'camera-fullscreen');
		this.notice = append(this.element, $('p.camera-notice', { role: 'status' }));
		this.updateControls();
		this.registerListeners();
		void this.start();
	}

	public static idFor(deviceId: string): string {
		return `${CameraView.PREFIX}${deviceId}`;
	}

	private registerListeners(): void {
		this._register(this.recording.onDidChange(() => {
			this.updateControls();
		}));
		this._register(this.recording.onDidEnd(status => {
			this.showNotice(status ? recordingEndMessage(status) : nls.localize('camera.recordingLost', 'The recording stopped.'));
		}));
		this._register(this.retry.onDidClick(() => {
			void this.start();
		}));
		this._register(addDisposableListener(this.picker, EventType.CHANGE, () => {
			lastCamera.set(this.device.id, this.picker.value);
			void this.open(this.picker.value);
		}));
		this._register(addDisposableListener(this.shutter, EventType.CLICK, () => {
			void this.takePicture();
		}));
		this._register(addDisposableListener(this.record, EventType.CLICK, () => {
			void this.toggleRecording();
		}));
		this._register(addDisposableListener(this.fit, EventType.CLICK, () => {
			this.toggleFit();
		}));
		this._register(addDisposableListener(this.fullScreenButton, EventType.CLICK, () => {
			void this.setFullScreen(!this.fullScreen);
		}));
		// Cmd/Ctrl+C reaches the page as a copy, from the Edit menu or the keyboard.
		this._register(addDisposableListener(this.element.ownerDocument, 'copy', event => {
			const target = event.target instanceof Node ? event.target : null;
			const here = target === this.element.ownerDocument.body || this.element.contains(target);
			const selection = this.element.ownerDocument.getSelection();
			if (this.tabService.activeTab === this && this.hasPicture && here && (!selection || selection.isCollapsed)) {
				event.preventDefault();
				void this.copy();
			}
		}));
		this._register(addDisposableListener(this.element.ownerDocument, EventType.KEY_DOWN, event => {
			if (event.key === 'Escape' && this.fullScreen && this.tabService.activeTab === this) {
				event.preventDefault();
				void this.setFullScreen(false);
			}
		}));
		for (const type of [EventType.MOUSE_MOVE, EventType.POINTER_DOWN, EventType.FOCUS_IN]) {
			this._register(addDisposableListener(this.element, type, () => {
				this.revealControls();
			}));
		}
		this._register(this.nativeHost.onDidChangeWindowState(state => {
			// The window left full screen another way, such as its green button.
			if (this.fullScreen && !state.fullscreen) {
				this.applyFullScreen(false);
			}
		}));
		this._register(toDisposable(() => {
			if (this.fullScreen) {
				void this.nativeHost.setFullScreen(false);
			}
		}));
	}

	/** Lists the cameras again and opens the chosen one. */
	private async start(): Promise<void> {
		const attempt = ++this.attempt;
		this.stream.clear();
		this.showStatus(nls.localize('camera.connecting', 'Connecting to the camera on {0}…', this.device.name));
		const result = await this.cameraService.cameras(this.device.id);
		if (attempt !== this.attempt || this._store.isDisposed) {
			return;
		}
		if ('failure' in result) {
			this.showProblem(result.failure);
			return;
		}
		this.cameras = result.cameras;
		this.recordingAllowed = result.recording;
		this.renderPicker();
		await this.open(this.picker.value, attempt);
	}

	private renderPicker(): void {
		const remembered = lastCamera.get(this.device.id);
		const chosen = this.cameras.find(camera => camera.id === remembered) ?? this.cameras[0];
		this.picker.replaceChildren(...this.cameras.map(camera => new Option(camera.name, camera.id, false, camera === chosen)));
		this.picker.hidden = this.cameras.length < 2;
	}

	private async open(cameraId: string, attempt = ++this.attempt): Promise<void> {
		this.stream.clear();
		this.setPlaying(false);
		if (!this.hasPicture) {
			this.showStatus(nls.localize('camera.connecting', 'Connecting to the camera on {0}…', this.device.name));
		}
		const result = await this.cameraService.open(this.device.id, cameraId);
		if (attempt !== this.attempt || this._store.isDisposed) {
			if ('stream' in result) {
				result.stream.dispose();
			}
			return;
		}
		if ('failure' in result) {
			this.showProblem(result.failure);
			return;
		}
		const stream = result.stream;
		stream.onFrame(frame => {
			if (!this.playing) {
				this.hideMessage();
				this.setPlaying(true);
			}
			this.draw(frame);
		});
		stream.onEnd(end => {
			this.showProblem(end);
		});
		this.stream.value = stream;
	}

	private draw(frame: VideoFrame): void {
		try {
			// A background tab keeps decoding, so showing it again is immediate, but draws nothing.
			if (!this.context || this.tabService.activeTab !== this) {
				return;
			}
			if (this.canvas.width !== frame.displayWidth || this.canvas.height !== frame.displayHeight) {
				this.canvas.width = frame.displayWidth;
				this.canvas.height = frame.displayHeight;
			}
			this.context.drawImage(frame, 0, 0);
			if (!this.hasPicture) {
				this.hasPicture = true;
				this.stage.classList.add('camera-stage-picture');
			}
		} finally {
			frame.close();
		}
	}

	private setPlaying(playing: boolean): void {
		this.playing = playing;
		this.element.classList.toggle('camera-playing', playing);
		this.updateControls();
		this.revealControls();
	}

	private showStatus(text: string): void {
		this.showMessage(text, false, false);
	}

	private showProblem(problem: CameraProblem): void {
		this.stream.clear();
		this.setPlaying(false);
		this.showMessage(problemMessage(this.device.name, problem), true, retryable.has(problem));
	}

	private showMessage(text: string, problem: boolean, retry: boolean): void {
		this.messageText.textContent = text;
		this.messageText.setAttribute('role', problem ? 'alert' : 'status');
		this.retry.element.hidden = !retry;
		this.message.hidden = false;
		// The last picture stays, dimmed, under what happened to it.
		this.stage.classList.toggle('camera-stage-dimmed', this.hasPicture);
	}

	private hideMessage(): void {
		this.message.hidden = true;
		this.stage.classList.remove('camera-stage-dimmed');
	}

	/** Shows the controls, and hides them again once a playing picture sits untouched. */
	private revealControls(): void {
		this.element.classList.remove('camera-idle');
		this.idleTimer.clear();
		if (!this.playing) {
			return;
		}
		const timer = setTimeout(() => {
			// Controls holding keyboard focus stay; hiding them would hide the focus.
			if (!this.controls.contains(this.element.ownerDocument.activeElement)) {
				this.element.classList.add('camera-idle');
			}
		}, controlsDelay);
		this.idleTimer.value = toDisposable(() => {
			clearTimeout(timer);
		});
	}

	private updateControls(): void {
		this.shutter.disabled = !this.playing;
		const recording = this.recording.recording;
		this.record.disabled = this.recording.stopping || (!recording && (!this.playing || !this.recordingAllowed));
		describe(this.record, recording
			? nls.localize('camera.stopRecording', 'Stop Recording')
			: this.playing && !this.recordingAllowed
				? recordingStartMessage(this.device.name, 'notAllowed')
				: nls.localize('camera.record', 'Record'), recording ? Icons.stop : Icons.recording);
		this.record.setAttribute('aria-pressed', String(recording));
		this.element.classList.toggle('camera-is-recording', recording);
		this.recordingBadge.hidden = !recording;
		this.recordingTime.textContent = formatElapsed(this.recording.elapsedMs);
		describe(this.fit, this.fitted ? nls.localize('camera.fill', 'Fill the Tab') : nls.localize('camera.fit', 'Show the Whole Picture'), this.fitted ? Icons.fillView : Icons.fitToView);
		describe(this.fullScreenButton, this.fullScreen ? nls.localize('camera.exitFullScreen', 'Exit Full Screen') : nls.localize('camera.fullScreen', 'Full Screen'), this.fullScreen ? Icons.exitFullScreen : Icons.fullScreen);
	}

	/** The picture fills the tab, as a camera app shows it; fitting shows all of it. */
	public toggleFit(): void {
		this.fitted = !this.fitted;
		this.stage.classList.toggle('camera-stage-fit', this.fitted);
		this.updateControls();
	}

	public async setFullScreen(fullScreen: boolean): Promise<void> {
		if (fullScreen === this.fullScreen) {
			return;
		}
		this.applyFullScreen(fullScreen);
		await this.nativeHost.setFullScreen(fullScreen);
	}

	private applyFullScreen(fullScreen: boolean): void {
		this.fullScreen = fullScreen;
		this.element.classList.toggle('camera-view-fullscreen', fullScreen);
		this.updateControls();
		this.revealControls();
	}

	/** The picture on screen, as PNG. */
	private async picture(): Promise<VSBuffer | undefined> {
		if (!this.hasPicture) {
			return undefined;
		}
		const blob = await new Promise<Blob | null>(resolve => {
			this.canvas.toBlob(resolve, 'image/png');
		});
		return blob ? VSBuffer.wrap(new Uint8Array(await blob.arrayBuffer())) : undefined;
	}

	/** Saves the picture on screen to the recordings folder, as a camera's shutter does. */
	public async takePicture(): Promise<void> {
		const png = await this.picture();
		if (!png) {
			return;
		}
		// Restart the flash for a shot taken during the last one.
		this.element.classList.remove('camera-flash');
		requestAnimationFrame(() => {
			this.element.classList.add('camera-flash');
		});
		try {
			await this.recordingService.savePicture(`${this.device.name.replace(/[\\/:\0<>"|?*]/g, '-')} ${timestamp(new Date())}.png`, png);
			this.showNotice(nls.localize('camera.pictureSaved', 'Picture saved to the recordings folder.'));
		} catch {
			this.showNotice(nls.localize('camera.pictureFailed', 'The picture couldn\'t be saved to the recordings folder.'));
		}
	}

	public async copy(): Promise<void> {
		const png = await this.picture();
		if (png) {
			await this.nativeHost.writeClipboardPicture(png);
			this.showNotice(nls.localize('camera.copied', 'Picture copied.'));
		}
	}

	public async toggleRecording(): Promise<void> {
		const failure = await this.recording.toggle();
		if (failure && !this._store.isDisposed) {
			this.showNotice(recordingStartMessage(this.device.name, failure));
		}
	}

	private showNotice(text: string): void {
		this.notice.textContent = text;
		this.notice.classList.add('camera-notice-visible');
		const timer = setTimeout(() => {
			this.notice.classList.remove('camera-notice-visible');
		}, noticeDelay);
		this.noticeTimer.value = toDisposable(() => {
			clearTimeout(timer);
		});
	}
}
