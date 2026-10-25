import { Emitter } from '../../src/base/common/event.js';
import { DisposableStore } from '../../src/base/common/lifecycle.js';
import type { DesktopCondition, DesktopMediaStatus, DesktopRateLimit } from '../../src/platform/engine/common/dataChannel.js';
import type { DisplayInfo } from '../../src/platform/engine/common/engine.js';
import { IConfigurationService } from '../../src/platform/configuration/common/configuration.js';
import { INativeHostService, type IWindowState } from '../../src/platform/native/common/native.js';
import { InstantiationService } from '../../src/platform/instantiation/common/instantiationService.js';
import { ServiceCollection } from '../../src/platform/instantiation/common/serviceCollection.js';
import '../../src/workbench/contrib/remoteDesktop/remoteDesktop.contribution.js';
import { OpenRemoteDesktopAction, ViewScreenAction } from '../../src/workbench/contrib/remoteDesktop/remoteDesktopActions.js';
import { RemoteDesktopView, shownSize } from '../../src/workbench/contrib/remoteDesktop/remoteDesktopView.js';
import { IDeviceService, type DeviceSummary } from '../../src/workbench/services/devices/deviceService.js';
import { IRemoteDesktopService, type DesktopEnd, type DesktopFailure, type DesktopInput, type DesktopPicture, type DesktopPictureRole, type DesktopPointer, type DesktopStats, type IDesktopSession } from '../../src/workbench/services/remoteDesktop/remoteDesktopService.js';
import { ITabService } from '../../src/workbench/services/tabs/tabService.js';
import { TabService } from '../../src/workbench/services/tabs/tabServiceImpl.js';

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

/** A display as the other computer arranges it: the second one, right of the first. */
const display: DisplayInfo = { id: '7', origin_x: 1920, origin_y: 0, width: 1920, height: 1080, rotation_deg: 0, scale_percent: 100, revision: '3' };

/** A session whose pictures, end and input the test plays and reads. */
class FakeSession implements IDesktopSession {
	private readonly pictures = new Emitter<DesktopPicture>();
	public readonly onPicture = this.pictures.event;
	private readonly ends = new Emitter<DesktopEnd>();
	public readonly onEnd = this.ends.event;
	private readonly pointers = new Emitter<DesktopPointer>();
	public readonly onPointer = this.pointers.event;
	public readonly sent: DesktopInput[] = [];
	public readonly presentedPictures: DesktopPicture[] = [];
	public readonly skippedPictures: DesktopPicture[] = [];
	public disposed = false;
	/** What the engine says is wrong with the picture, most severe first. */
	public conditions: DesktopCondition[] = ['link.congestion', 'relay.path'];
	/** Why the quality is where it is, and what holds the bitrate. */
	public reason: DesktopMediaStatus['reason'] = 'networkPressure';
	public limit: DesktopRateLimit = 'none';

	constructor(public readonly control: boolean) { }

	public displays(): Promise<readonly DisplayInfo[]> {
		return Promise.resolve([display]);
	}

	public stats(): Promise<DesktopStats | undefined> {
		return Promise.resolve({ codec: 'h264', status: { reason: this.reason, receivedKbps: 9400, receivedFps: 60, receiverQueueMs: 4, hostQueueMs: 2, lossPerMille: 3, rttMs: 18, encodeUs: 4000, captureUs: 1000, limit: this.limit, conditions: this.conditions }, latency: { host: { p50: 9, p95: 14, count: 60 }, decode: { p50: 3, p95: 5, count: 60 } }, skipped: 0 });
	}

	public send(input: DesktopInput): void {
		if (this.control) {
			this.sent.push(input);
		}
	}

	public presented(picture: DesktopPicture): void {
		this.presentedPictures.push(picture);
	}

	public skipped(picture: DesktopPicture): void {
		this.skippedPictures.push(picture);
	}

	/** A 1920 × 1080 picture of display 7 at revision 3, in one colour, with a 40 × 40 patch of another at 100, 100. */
	public picture(role: DesktopPictureRole = 'changed', colour = '#1d4b6c', patch?: string): void {
		const canvas = new OffscreenCanvas(1920, 1080);
		const context = canvas.getContext('2d');
		if (context) {
			context.fillStyle = colour;
			context.fillRect(0, 0, 1920, 1080);
			if (patch) {
				context.fillStyle = patch;
				context.fillRect(100, 100, 40, 40);
			}
		}
		this.pictures.fire({ frame: new VideoFrame(canvas, { timestamp: 0 }), role, display: '7', revision: '3', decodedAt: performance.now() });
	}

	/** The other computer's pointer at `x`, `y` in the picture: a white 4 × 4 square, its hot spot at the top left. */
	public pointer(x: number, y: number, visible = true): void {
		const shape = { width: 4, height: 4, hotspotX: 0, hotspotY: 0, rgba: new Uint8ClampedArray(64).fill(255) };
		this.pointers.fire({ x, y, visible, shape });
	}

	public end(reason: DesktopEnd): void {
		this.ends.fire(reason);
	}

	public dispose(): void {
		this.disposed = true;
	}
}

export async function verifyRemoteDesktopSession(): Promise<void> {
	const disposables = new DisposableStore();
	const device: DeviceSummary = { id: 'studio', name: 'Studio PC', os: 'windows', presence: 'connected', hostname: 'studio-pc', direct: 1, relays: 0 };
	const sessions: FakeSession[] = [];
	const answers: (DesktopFailure | undefined)[] = [];
	const tabService = disposables.add(new TabService());
	const windowState = disposables.add(new Emitter<IWindowState>());
	const fullScreen: boolean[] = [];
	const services = disposables.add(new InstantiationService(new ServiceCollection(
		[ITabService, tabService],
		[INativeHostService, {
			_serviceBrand: undefined,
			onDidChangeWindowState: windowState.event,
			setFullScreen: (value: boolean) => {
				fullScreen.push(value);
				return Promise.resolve();
			},
		}],
		[IConfigurationService, { _serviceBrand: undefined, getValue: (key: string) => key === 'desktop.pictureSize' ? 'fit' : undefined }],
		[IDeviceService, { _serviceBrand: undefined, devices: [device] }],
		[IRemoteDesktopService, {
			_serviceBrand: undefined,
			open: (_deviceId: string, control: boolean) => {
				const failure = answers.shift();
				if (failure) {
					return Promise.resolve({ failure });
				}
				const session = new FakeSession(control);
				sessions.push(session);
				return Promise.resolve({ session });
			},
		}],
	), true));
	const host = document.createElement('div');
	host.style.cssText = 'position:fixed;left:0;top:40px;width:1280px;height:720px;z-index:99;display:flex;';
	document.body.append(host);
	const press = (target: HTMLElement, type: 'keydown' | 'keyup', code: string, key: string, init: KeyboardEventInit = {}): void => {
		target.dispatchEvent(new KeyboardEvent(type, { code, key, bubbles: true, cancelable: true, ...init }));
	};
	try {
		services.invokeFunction(accessor => {
			new OpenRemoteDesktopAction().run(accessor, device.id);
		});
		const view = tabService.activeTab;
		assert(view instanceof RemoteDesktopView && view.id === 'desktop:studio' && view.label === 'Studio PC' && view.control, 'Remote desktop did not open a tab for the computer');
		host.append(view.element);
		const overlay = view.element.querySelector<HTMLElement>('.desktop-overlay');
		const canvas = view.element.querySelector<HTMLCanvasElement>('.desktop-screen');
		const role = (): string => view.element.querySelector('.desktop-status-role')?.textContent ?? '';
		assert(overlay !== null && canvas !== null, 'The tab has no screen');
		// Connecting until the first picture, which then fills the tab and takes the keyboard.
		await until(() => sessions.length === 1, 'The tab did not open the screen');
		assert(!overlay.hidden && overlay.textContent.includes('Connecting to Studio PC'), 'Connecting is not said');
		sessions[0].picture();
		await until(() => overlay.hidden === true && role() === 'Controlling Studio PC', 'The first picture did not show');
		await until(() => view.element.querySelector('.desktop-status-size')?.textContent === '1920 × 1080', 'The display size is not shown');
		assert(canvas.width === 1920 && canvas.height === 1080, 'The picture was not drawn at its size');
		assert(document.activeElement === canvas, 'The screen did not take the keyboard');
		// Pictures decoded within one frame of the screen: only the newest is drawn.
		const presented = sessions[0].presentedPictures.length;
		sessions[0].picture();
		sessions[0].picture();
		sessions[0].picture();
		await until(() => sessions[0].presentedPictures.length === presented + 1, 'The newest picture was not drawn');
		await new Promise(resolve => requestAnimationFrame(resolve));
		assert(sessions[0].presentedPictures.length === presented + 1 && sessions[0].skippedPictures.length === 2, `Pictures behind the newest were drawn: ${String(sessions[0].presentedPictures.length - presented)} drawn, ${String(sessions[0].skippedPictures.length)} skipped`);
		// A still picture stays until the screen changes: the stream's pictures of the same screen are not drawn over it.
		const shownColour = (x = 960, y = 540): string => {
			const [red, green, blue] = canvas.getContext('2d')?.getImageData(x, y, 1, 1).data ?? [];
			return `${String(red)},${String(green)},${String(blue)}`;
		};
		const frame = (): Promise<unknown> => new Promise(resolve => requestAnimationFrame(resolve));
		// The stream's repeat of the screen the still will show, to compare changes with.
		sessions[0].picture('unchanged', '#808080');
		await frame();
		await frame();
		sessions[0].picture('still', '#ff8000');
		await frame();
		await frame();
		assert(shownColour() === '255,128,0', `The still was not drawn: ${shownColour()}`);
		sessions[0].picture('unchanged', '#808080');
		await frame();
		await frame();
		assert(shownColour() === '255,128,0', `A picture of the same screen was drawn over the still: ${shownColour()}`);
		// A small change, such as a clock's, redraws only its tiles: the still stays everywhere else.
		sessions[0].picture('changed', '#808080', '#00ff00');
		await frame();
		await frame();
		assert(shownColour(110, 110) === '0,255,0', `A small change was not drawn: ${shownColour(110, 110)}`);
		assert(shownColour() === '255,128,0' && shownColour(300, 300) === '255,128,0', `A small change replaced the still: ${shownColour()}, ${shownColour(300, 300)}`);
		// Repeats after it keep both: the changed tiles from the stream, the rest from the still.
		sessions[0].picture('unchanged', '#808080', '#00ff00');
		await frame();
		await frame();
		assert(shownColour(110, 110) === '0,255,0' && shownColour() === '255,128,0', 'A repeat after a small change undid it');
		// Most of the screen changing replaces the still everywhere.
		sessions[0].picture('changed', '#0000ff');
		await frame();
		await frame();
		assert(shownColour() === '0,0,255', `A changed screen did not replace the still: ${shownColour()}`);
		// How the picture is doing, once a second.
		await until(() => view.element.querySelector('.desktop-status-stats')?.textContent === '60 fps · H.264 · 9.4 Mb/s · 18 ms · Lowered for the network', `Wrong stats: ${view.element.querySelector('.desktop-status-stats')?.textContent ?? ''}`, 3000);
		// A bitrate that is held says what holds it, and only when no cut already explains the picture.
		sessions[0].reason = 'stable';
		for (const [limit, text] of [['setting', 'At your bandwidth limit'], ['relay', 'Limited by the relay'], ['network', 'Limited by the network'], ['encoder', 'Limited by Studio PC'], ['receiver', 'Limited by this computer']] as const) {
			sessions[0].limit = limit;
			await until(() => view.element.querySelector('.desktop-status-stats')?.textContent === `60 fps · H.264 · 9.4 Mb/s · 18 ms · ${text}`, `Wrong stats for ${limit}: ${view.element.querySelector('.desktop-status-stats')?.textContent ?? ''}`, 3000);
		}
		sessions[0].limit = 'none';
		await until(() => view.element.querySelector('.desktop-status-stats')?.textContent === '60 fps · H.264 · 9.4 Mb/s · 18 ms', 'A bitrate nothing holds was explained', 3000);
		sessions[0].reason = 'networkPressure';
		sessions[0].limit = 'network';
		await until(() => view.element.querySelector('.desktop-status-stats')?.textContent === '60 fps · H.264 · 9.4 Mb/s · 18 ms · Lowered for the network', 'A cut said its cause twice', 3000);
		sessions[0].limit = 'none';
		// What is wrong, beside the stats: the worst in one phrase, all of them on hover and to a screen reader.
		const condition = view.element.querySelector<HTMLElement>('.desktop-status-condition');
		assert(condition !== null && !condition.hidden && condition.textContent === 'Slow network: lowered quality', `Wrong condition: ${condition?.textContent ?? ''}`);
		assert(condition.title === 'Slow network: lowered quality\nConnected through a relay' && condition.tabIndex === 0, `Wrong condition details: ${condition.title}`);
		assert(condition.getAttribute('aria-label') === 'Slow network: lowered quality. Connected through a relay', 'The conditions are not read out');
		// A relay alone needs no attention.
		sessions[0].conditions = ['relay.path'];
		await until(() => condition.hidden === true, 'A relay alone was shown as a problem', 3000);
		sessions[0].conditions = ['encoder.software'];
		await until(() => condition.textContent === 'Studio PC is encoding in software', `Wrong software condition: ${condition.textContent}`, 3000);

		// The pointer's display is chosen once, before anything else is sent.
		assert(JSON.stringify(sessions[0].sent[0]) === JSON.stringify({ type: 'selectDisplays', ids: ['7'] }), `The shown display was not selected: ${JSON.stringify(sessions[0].sent[0])}`);
		// Keys go by where they are, with what they type; a held key repeats; a shortcut types nothing; a dead key composes.
		press(canvas, 'keydown', 'KeyA', 'a');
		press(canvas, 'keydown', 'KeyA', 'a', { repeat: true });
		press(canvas, 'keyup', 'KeyA', 'a');
		press(canvas, 'keydown', 'KeyC', 'c', { metaKey: true });
		press(canvas, 'keyup', 'KeyC', 'c');
		press(canvas, 'keydown', 'BracketLeft', 'Dead');
		press(canvas, 'keyup', 'BracketLeft', 'Dead');
		const key = (usage: number, down: boolean, text = '', composing = false) => ({ type: 'submitLocalKey', usage, down, text, composing });
		const keys = sessions[0].sent.filter(input => input.type === 'submitLocalKey');
		assert(JSON.stringify(keys) === JSON.stringify([
			key(4, true, 'a'), key(4, true, 'a'), key(4, false),
			key(6, true), key(6, false),
			key(0x2f, true, '', true), key(0x2f, false),
		]), `Wrong keys: ${JSON.stringify(keys)}`);
		// On a Mac, Option makes characters, as on a French keyboard Option-à types @.
		if (navigator.platform.startsWith('Mac')) {
			press(canvas, 'keydown', 'Digit0', '@', { altKey: true });
			assert(JSON.stringify(sessions[0].sent.at(-1)) === JSON.stringify(key(0x27, true, '@')), `Option-typed text was lost: ${JSON.stringify(sessions[0].sent.at(-1))}`);
			press(canvas, 'keyup', 'Digit0', '@', { altKey: true });
		}

		// Fitted by default: its own pixels, smaller when the tab can't hold it, centred.
		const stage = view.element.querySelector<HTMLElement>('.desktop-stage');
		assert(stage !== null, 'The tab has no stage');
		const ratio = window.devicePixelRatio || 1;
		const room = { width: stage.clientWidth, height: stage.clientHeight };
		const fitted = shownSize('fit', 1920, 1080, room, ratio);
		const box = canvas.getBoundingClientRect();
		assert(Math.abs(box.width - fitted.width) < 0.5 && Math.abs(box.height - fitted.height) < 0.5 && fitted.width * ratio <= 1920, `The picture was not fitted: ${String(box.width)} × ${String(box.height)}`);
		const stageBox = stage.getBoundingClientRect();
		assert(Math.abs(box.left - stageBox.left - (room.width - box.width) / 2) < 1 && Math.abs(box.top - stageBox.top - (room.height - box.height) / 2) < 1, 'The fitted picture is not centred');
		// The pointer lands on the other computer's display, through the picture as shown.
		const center = { clientX: box.left + box.width / 2, clientY: box.top + box.height / 2, bubbles: true, cancelable: true, pointerId: 1 };
		canvas.dispatchEvent(new PointerEvent('pointermove', center));
		canvas.dispatchEvent(new PointerEvent('pointerdown', { ...center, button: 0 }));
		canvas.dispatchEvent(new PointerEvent('pointerup', { ...center, button: 0 }));
		const pointer = sessions[0].sent.find(input => input.type === 'submitPointer');
		assert(pointer?.type === 'submitPointer' && pointer.x === 1920 + 960 && pointer.y === 540 && pointer.display === '7' && pointer.revision === '3', `Wrong pointer: ${JSON.stringify(pointer)}`);
		assert(sessions[0].sent.filter(input => input.type === 'submitButton').map(input => JSON.stringify(input)).join() === [
			JSON.stringify({ type: 'submitButton', button: 'Left', down: true }), JSON.stringify({ type: 'submitButton', button: 'Left', down: false }),
		].join(), 'The click did not reach the other computer');
		// Above the picture, as a drag captured from it reports, the pointer goes nowhere.
		const before = sessions[0].sent.length;
		canvas.dispatchEvent(new PointerEvent('pointermove', { ...center, clientY: box.top - 2 }));
		assert(sessions[0].sent.length === before, 'The pointer was sent from outside the picture');
		// A notch of the wheel is 120 of the engine's units.
		canvas.dispatchEvent(new WheelEvent('wheel', { deltaY: 100, bubbles: true, cancelable: true }));
		assert(JSON.stringify(sessions[0].sent.at(-1)) === JSON.stringify({ type: 'submitPreciseWheel', dx: 0, dy: 120 }), `Wrong wheel: ${JSON.stringify(sessions[0].sent.at(-1))}`);
		// The other computer's pointer: while this one is over the picture, it takes that pointer's shape;
		// away from the picture, the other pointer is drawn where it is.
		const mark = view.element.querySelector<HTMLElement>('.desktop-pointer');
		assert(mark !== null, 'The tab has no place for the other pointer');
		canvas.dispatchEvent(new MouseEvent('mouseenter'));
		sessions[0].pointer(960, 540);
		assert(canvas.style.cursor.includes('image-set') && mark.hidden === true, `This computer's pointer did not take the other's shape: ${canvas.style.cursor}`);
		canvas.dispatchEvent(new MouseEvent('mouseleave'));
		const drawn = (): boolean => mark.hidden !== true;
		assert(drawn(), 'The other pointer was not drawn while this one is away');
		const scale = box.width / 1920;
		const placed = mark.getBoundingClientRect();
		assert(Math.abs(placed.left - (box.left + 960 * scale)) < 1.5 && Math.abs(placed.top - (box.top + 540 * scale)) < 1.5, `The other pointer is in the wrong place: ${String(placed.left)}, ${String(placed.top)}`);
		// Original size: one picture pixel on one screen pixel, scrolling where the tab is smaller.
		const picker = view.element.querySelector<HTMLSelectElement>('.desktop-size-picker select');
		assert(picker !== null && picker.value === 'fit' && picker.getAttribute('aria-label') === 'Picture size', 'The picture size choice is missing');
		assert([...picker.options].map(option => option.textContent).join() === 'Fit,Stretch,50%,75%,Original size,150%,200%', `Wrong picture sizes: ${[...picker.options].map(option => option.textContent).join()}`);
		const choose = (value: string): void => {
			picker.value = value;
			picker.dispatchEvent(new Event('change'));
		};
		choose('1');
		const original = canvas.getBoundingClientRect();
		assert(Math.abs(original.width * ratio - 1920) < 0.5 && Math.abs(original.height * ratio - 1080) < 0.5, `Original size is not one pixel per pixel: ${String(original.width * ratio)}`);
		const scrolls = stage.scrollWidth > stage.clientWidth;
		assert(scrolls === original.width > room.width, 'The stage does not scroll exactly when the picture is larger');
		if (scrolls) {
			stage.scrollLeft = 200;
			stage.dispatchEvent(new Event('scroll'));
		}
		// The pointer follows the scrolled picture: what is under it is what is sent.
		const shown = canvas.getBoundingClientRect();
		const inside = { clientX: Math.max(shown.left, stageBox.left) + 100, clientY: Math.max(shown.top, stageBox.top) + 100 };
		const at = { ...inside, bubbles: true, cancelable: true, pointerId: 1 };
		canvas.dispatchEvent(new PointerEvent('pointermove', at));
		const scrolled = sessions[0].sent.at(-1);
		const expectX = 1920 + Math.floor((at.clientX - shown.left) / shown.width * 1920);
		assert(scrolled?.type === 'submitPointer' && scrolled.x === expectX, `The pointer missed the scrolled picture: ${JSON.stringify(scrolled)}, expected x ${String(expectX)}`);
		const markAt = mark.getBoundingClientRect();
		assert(Math.abs(markAt.left - (shown.left + 960 * shown.width / 1920)) < 1.5, `The other pointer did not follow the scrolled picture: ${String(markAt.left)}`);
		// Stretch fills the tab both ways it can, keeping the picture's shape.
		choose('stretch');
		const stretched = canvas.getBoundingClientRect();
		assert(Math.abs(Math.min(room.width - stretched.width, room.height - stretched.height)) < 1 && stage.scrollWidth <= stage.clientWidth, `Stretch did not fill the tab: ${String(stretched.width)} × ${String(stretched.height)}`);
		choose('fit');
		// Full screen covers the window and is left from its button or the window itself, not by a key, which goes to the other computer.
		const toggle = view.element.querySelector<HTMLButtonElement>('.desktop-status-button');
		assert(toggle?.getAttribute('aria-label') === 'Full Screen', 'The full screen button is missing');
		toggle.click();
		await until(() => fullScreen.join() === 'true' && view.element.classList.contains('desktop-view-fullscreen') && toggle.getAttribute('aria-label') === 'Exit Full Screen', 'Full screen did not cover the window');
		toggle.click();
		await until(() => fullScreen.join() === 'true,false' && !view.element.classList.contains('desktop-view-fullscreen'), 'The button did not leave full screen');
		toggle.click();
		windowState.fire({ windowId: 1, revision: 1, focused: true, maximized: false, fullscreen: false, alwaysOnTop: false, zoomFactor: 1, backgroundMaterial: 'none' });
		assert(!view.element.classList.contains('desktop-view-fullscreen') && toggle.getAttribute('aria-label') === 'Full Screen', 'Leaving full screen from the window did not restore the tab');
		sessions[0].pointer(960, 540, false);
		assert(!drawn() && canvas.style.cursor === 'none', 'A hidden pointer still shows');
		// Leaving the screen lets go of everything held there.
		// A test window has no focus of its own to lose, so the blur is dispatched as the system would.
		canvas.dispatchEvent(new FocusEvent('blur'));
		assert(sessions[0].sent.at(-1)?.type === 'releaseSessionInput', 'Leaving the screen did not let go of the keys');

		// A dropped connection is tried again by itself, over the last picture, dimmed.
		sessions[0].end('linkLost');
		await until(() => !overlay.hidden && overlay.textContent.includes('Reconnecting to Studio PC'), 'A lost connection did not say it reconnects');
		assert(view.element.querySelector('.desktop-stage')?.classList.contains('desktop-dimmed') === true, 'The last picture was not dimmed');
		await until(() => sessions.length === 2, 'The tab did not reconnect by itself', 4000);
		sessions[1].picture();
		await until(() => overlay.hidden === true, 'The reconnected picture did not show');
		// The other computer's encoder failed: it reconnects at once, and says whose failure it was.
		sessions[1].end('hostEncoderFailed');
		await until(() => !overlay.hidden && overlay.textContent.includes('Studio PC couldn’t keep encoding its screen.'), `An encoder failure was not said: ${overlay.textContent}`);
		await until(() => sessions.length === 3, 'The tab did not reconnect after an encoder failure', 4000);
		sessions[2].picture();
		await until(() => overlay.hidden === true, 'The picture after an encoder failure did not show');
		// Permission withdrawn there: not tried again; it says what someone there must do.
		sessions[2].end('hostPermission');
		await until(() => overlay.textContent.includes('Studio PC no longer allows VniLink to record its screen.') && overlay.textContent.includes('system settings'), `A withdrawn permission was not said: ${overlay.textContent}`);
		// An end interrupts: its overlay is an alert, and the picture's conditions go with it.
		assert(overlay.getAttribute('role') === 'alert', 'An end is not announced as an alert');
		assert(view.element.querySelector<HTMLElement>('.desktop-status-condition')?.hidden === true, 'A condition outlived its picture');
		const opened = sessions.length;
		assert(opened === 3, 'A withdrawn permission was tried again by itself');
		overlay.querySelector<HTMLButtonElement>('button.primary')?.click();
		await until(() => sessions.length === 4, 'Try Again did not open the screen after a withdrawn permission');
		sessions[3].picture();
		await until(() => overlay.hidden === true, 'The picture after Try Again did not show');
		// Refused for good: it says why, and offers to try again or close.
		answers.push('turnedOff');
		sessions[3].end('linkLost');
		await until(() => overlay.textContent.includes('Studio PC doesn’t share its screen with this computer.'), 'A refusal is not said', 4000);
		const buttons = [...overlay.querySelectorAll('button')].map(button => button.textContent);
		assert(buttons.join() === 'Try Again,Close Tab', `Wrong actions: ${buttons.join()}`);
		assert(role() === 'Not connected', 'The status bar does not say it is not connected');
		overlay.querySelector<HTMLButtonElement>('button.primary')?.click();
		await until(() => sessions.length === 5, 'Try Again did not open the screen');
		assert(await tabService.close(view.id), 'The tab did not close');
		assert(sessions[4].disposed, 'Closing the tab did not end the session');

		// Viewing only sends nothing.
		services.invokeFunction(accessor => {
			new ViewScreenAction().run(accessor, device.id);
		});
		const viewer = tabService.activeTab;
		assert(viewer instanceof RemoteDesktopView && !viewer.control, 'View Screen did not open a view-only tab');
		host.replaceChildren(viewer.element);
		await until(() => sessions.length === 6, 'The view-only tab did not open the screen');
		sessions[5].picture();
		await until(() => viewer.element.querySelector('.desktop-status-role')?.textContent === 'Viewing Studio PC', 'Viewing is not said');
		const screen = viewer.element.querySelector<HTMLCanvasElement>('.desktop-screen');
		assert(screen !== null, 'The view-only tab has no screen');
		press(screen, 'keydown', 'KeyA', 'a');
		assert(sessions[5].sent.length === 0, 'A view-only tab sent input');
		// Viewing, the other pointer is drawn even under this one, which keeps its own shape.
		screen.dispatchEvent(new MouseEvent('mouseenter'));
		sessions[5].pointer(100, 100);
		const viewed = viewer.element.querySelector<HTMLElement>('.desktop-pointer');
		assert(viewed?.hidden === false && screen.style.cursor === '', 'Viewing did not draw the other pointer');
		// A picture this computer can't show: once it reconnects by itself, as the
		// next offer leaves that codec out; the second time it says so.
		sessions[5].end('cantShow');
		await until(() => sessions.length === 7, 'The tab did not reconnect after a picture it could not show');
		sessions[6].end('cantShow');
		const viewerOverlay = viewer.element.querySelector<HTMLElement>('.desktop-overlay');
		await until(() => viewerOverlay?.textContent.includes('This computer can’t show the screen of Studio PC.') === true, 'A picture it can never show is not said');
		assert(sessions.length === 7, 'The tab kept reconnecting to a screen it cannot show');
	} finally {
		host.remove();
		disposables.dispose();
	}
}
