import { Emitter } from '../../src/base/common/event.js';
import { DisposableStore } from '../../src/base/common/lifecycle.js';
import type { DisplayInfo } from '../../src/platform/engine/common/engine.js';
import { InstantiationService } from '../../src/platform/instantiation/common/instantiationService.js';
import { ServiceCollection } from '../../src/platform/instantiation/common/serviceCollection.js';
import '../../src/workbench/contrib/remoteDesktop/remoteDesktop.contribution.js';
import { OpenRemoteDesktopAction, ViewScreenAction } from '../../src/workbench/contrib/remoteDesktop/remoteDesktopActions.js';
import { RemoteDesktopView } from '../../src/workbench/contrib/remoteDesktop/remoteDesktopView.js';
import { IDeviceService, type DeviceSummary } from '../../src/workbench/services/devices/deviceService.js';
import { IRemoteDesktopService, type DesktopEnd, type DesktopFailure, type DesktopInput, type DesktopPicture, type DesktopPointer, type DesktopStats, type IDesktopSession } from '../../src/workbench/services/remoteDesktop/remoteDesktopService.js';
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

	constructor(public readonly control: boolean) { }

	public displays(): Promise<readonly DisplayInfo[]> {
		return Promise.resolve([display]);
	}

	public stats(): Promise<DesktopStats | undefined> {
		return Promise.resolve({ codec: 'h264', status: { reason: 'networkPressure', receivedKbps: 9400, receivedFps: 60, receiverQueueMs: 4, hostQueueMs: 2, lossPerMille: 3, rttMs: 18, encodeUs: 4000, captureUs: 1000 }, latency: { host: { p50: 9, p95: 14, count: 60 }, decode: { p50: 3, p95: 5, count: 60 } }, skipped: 0 });
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

	/** A 1920 × 1080 picture of display 7 at revision 3. */
	public picture(): void {
		const canvas = new OffscreenCanvas(1920, 1080);
		const context = canvas.getContext('2d');
		if (context) {
			context.fillStyle = '#1d4b6c';
			context.fillRect(0, 0, 1920, 1080);
		}
		this.pictures.fire({ frame: new VideoFrame(canvas, { timestamp: 0 }), display: '7', revision: '3', decodedAt: performance.now() });
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
	const device: DeviceSummary = { id: 'studio', name: 'Sudosy', os: 'windows', presence: 'connected', hostname: 'sudosy', direct: 1, relays: 0 };
	const sessions: FakeSession[] = [];
	const answers: (DesktopFailure | undefined)[] = [];
	const tabService = disposables.add(new TabService());
	const services = disposables.add(new InstantiationService(new ServiceCollection(
		[ITabService, tabService],
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
		assert(view instanceof RemoteDesktopView && view.id === 'desktop:studio' && view.label === 'Sudosy' && view.control, 'Remote desktop did not open a tab for the computer');
		host.append(view.element);
		const overlay = view.element.querySelector<HTMLElement>('.desktop-overlay');
		const canvas = view.element.querySelector<HTMLCanvasElement>('.desktop-screen');
		const role = (): string => view.element.querySelector('.desktop-status-role')?.textContent ?? '';
		assert(overlay !== null && canvas !== null, 'The tab has no screen');
		// Connecting until the first picture, which then fills the tab and takes the keyboard.
		await until(() => sessions.length === 1, 'The tab did not open the screen');
		assert(!overlay.hidden && overlay.textContent.includes('Connecting to Sudosy'), 'Connecting is not said');
		sessions[0].picture();
		await until(() => overlay.hidden === true && role() === 'Controlling Sudosy', 'The first picture did not show');
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
		// How the picture is doing, once a second.
		await until(() => view.element.querySelector('.desktop-status-stats')?.textContent === '60 fps · H.264 · 9.4 Mb/s · 18 ms · Lowered for the network', `Wrong stats: ${view.element.querySelector('.desktop-status-stats')?.textContent ?? ''}`, 3000);

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

		// The pointer lands on the other computer's display, through the fitted picture.
		const box = canvas.getBoundingClientRect();
		const center = { clientX: box.left + box.width / 2, clientY: box.top + box.height / 2, bubbles: true, cancelable: true, pointerId: 1 };
		canvas.dispatchEvent(new PointerEvent('pointermove', center));
		canvas.dispatchEvent(new PointerEvent('pointerdown', { ...center, button: 0 }));
		canvas.dispatchEvent(new PointerEvent('pointerup', { ...center, button: 0 }));
		const pointer = sessions[0].sent.find(input => input.type === 'submitPointer');
		assert(pointer?.type === 'submitPointer' && pointer.x === 1920 + 960 && pointer.y === 540 && pointer.display === '7' && pointer.revision === '3', `Wrong pointer: ${JSON.stringify(pointer)}`);
		assert(sessions[0].sent.filter(input => input.type === 'submitButton').map(input => JSON.stringify(input)).join() === [
			JSON.stringify({ type: 'submitButton', button: 'Left', down: true }), JSON.stringify({ type: 'submitButton', button: 'Left', down: false }),
		].join(), 'The click did not reach the other computer');
		// Above the picture, in the margin, the pointer goes nowhere.
		const before = sessions[0].sent.length;
		canvas.dispatchEvent(new PointerEvent('pointermove', { ...center, clientY: box.top + 1 }));
		assert(sessions[0].sent.length === before || box.height <= box.width * 1080 / 1920, 'The pointer was sent from outside the picture');
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
		const scale = Math.min(box.width / 1920, box.height / 1080);
		const placed = mark.getBoundingClientRect();
		assert(Math.abs(placed.left - (box.left + (box.width - 1920 * scale) / 2 + 960 * scale)) < 1.5
			&& Math.abs(placed.top - (box.top + (box.height - 1080 * scale) / 2 + 540 * scale)) < 1.5, `The other pointer is in the wrong place: ${String(placed.left)}, ${String(placed.top)}`);
		sessions[0].pointer(960, 540, false);
		assert(!drawn() && canvas.style.cursor === 'none', 'A hidden pointer still shows');
		// Leaving the screen lets go of everything held there.
		// A test window has no focus of its own to lose, so the blur is dispatched as the system would.
		canvas.dispatchEvent(new FocusEvent('blur'));
		assert(sessions[0].sent.at(-1)?.type === 'releaseSessionInput', 'Leaving the screen did not let go of the keys');

		// A dropped connection is tried again by itself, over the last picture, dimmed.
		sessions[0].end('lost');
		await until(() => !overlay.hidden && overlay.textContent.includes('Reconnecting to Sudosy'), 'A lost connection did not say it reconnects');
		assert(view.element.querySelector('.desktop-stage')?.classList.contains('desktop-dimmed') === true, 'The last picture was not dimmed');
		await until(() => sessions.length === 2, 'The tab did not reconnect by itself', 4000);
		sessions[1].picture();
		await until(() => overlay.hidden === true, 'The reconnected picture did not show');
		// Refused for good: it says why, and offers to try again or close.
		answers.push('turnedOff');
		sessions[1].end('lost');
		await until(() => overlay.textContent.includes('Sudosy doesn’t share its screen with this computer.'), 'A refusal is not said', 4000);
		const buttons = [...overlay.querySelectorAll('button')].map(button => button.textContent);
		assert(buttons.join() === 'Try Again,Close Tab', `Wrong actions: ${buttons.join()}`);
		assert(role() === 'Not connected', 'The status bar does not say it is not connected');
		overlay.querySelector<HTMLButtonElement>('button.primary')?.click();
		await until(() => sessions.length === 3, 'Try Again did not open the screen');
		assert(await tabService.close(view.id), 'The tab did not close');
		assert(sessions[2].disposed, 'Closing the tab did not end the session');

		// Viewing only sends nothing.
		services.invokeFunction(accessor => {
			new ViewScreenAction().run(accessor, device.id);
		});
		const viewer = tabService.activeTab;
		assert(viewer instanceof RemoteDesktopView && !viewer.control, 'View Screen did not open a view-only tab');
		host.replaceChildren(viewer.element);
		await until(() => sessions.length === 4, 'The view-only tab did not open the screen');
		sessions[3].picture();
		await until(() => viewer.element.querySelector('.desktop-status-role')?.textContent === 'Viewing Sudosy', 'Viewing is not said');
		const screen = viewer.element.querySelector<HTMLCanvasElement>('.desktop-screen');
		assert(screen !== null, 'The view-only tab has no screen');
		press(screen, 'keydown', 'KeyA', 'a');
		assert(sessions[3].sent.length === 0, 'A view-only tab sent input');
		// Viewing, the other pointer is drawn even under this one, which keeps its own shape.
		screen.dispatchEvent(new MouseEvent('mouseenter'));
		sessions[3].pointer(100, 100);
		const viewed = viewer.element.querySelector<HTMLElement>('.desktop-pointer');
		assert(viewed?.hidden === false && screen.style.cursor === '', 'Viewing did not draw the other pointer');
	} finally {
		host.remove();
		disposables.dispose();
	}
}
