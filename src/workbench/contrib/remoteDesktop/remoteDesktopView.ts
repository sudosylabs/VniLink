import * as nls from '../../../nls.js';
import { $, addDisposableListener, append, DisposableResizeObserver, EventType, getWindow } from '../../../base/browser/dom.js';
import { status as announce } from '../../../base/browser/ui/aria/aria.js';
import { Icons } from '../../../base/common/icons.js';
import { isMacintosh, isWindows } from '../../../base/common/platform.js';
import { Disposable, DisposableStore, MutableDisposable, toDisposable } from '../../../base/common/lifecycle.js';
import type { DesktopCondition } from '../../../platform/engine/common/dataChannel.js';
import type { DisplayInfo } from '../../../platform/engine/common/engine.js';
import { hidUsage } from '../../../platform/engine/common/keyUsage.js';
import type { DeviceSummary } from '../../services/devices/deviceService.js';
import { latencyStages, type LatencyStage } from '../../services/remoteDesktop/desktopLatency.js';
import { IRemoteDesktopService, type DesktopEnd, type DesktopFailure, type DesktopInput, type DesktopPicture, type DesktopPointer, type DesktopPointerShape, type DesktopStats, type IDesktopSession } from '../../services/remoteDesktop/remoteDesktopService.js';
import { ITabService, type WorkbenchTab } from '../../services/tabs/tabService.js';

type DesktopProblem = DesktopFailure | DesktopEnd;

/** Problems another attempt may fix, tried again by themselves; the rest need a change on either computer. */
const retryable: ReadonlySet<DesktopProblem> = new Set(['disconnected', 'unavailable', 'failed', 'linkLost', 'relayUnreachable', 'peerClosed', 'hostEncoderFailed', 'hostCaptureFailed']);

/** Ends of a running picture that are tried again at once, before any failed attempt. */
const reconnectAtOnce: ReadonlySet<DesktopProblem> = new Set(['linkLost', 'relayUnreachable', 'peerClosed', 'hostEncoderFailed', 'hostCaptureFailed']);

/** Precise wheel units in one notch, the Windows wheel delta, as the engine counts them. */
const WHEEL_NOTCH = 120;
/** A notch scrolls 100 pixels in Chromium; the engine's limit per event is 64 notches. */
const WHEEL_PER_PIXEL = WHEEL_NOTCH / 100;

const WHEEL_LIMIT = WHEEL_NOTCH * 64;

const buttons = ['Left', 'Middle', 'Right', 'Back', 'Forward'] as const;

/** The HID usage of the left Control key, which Windows also presses for AltGr. */
const CONTROL_LEFT = 0xe0;

interface OverlayAction {
	readonly label: string;
	readonly primary?: boolean;
	readonly run: () => void;
}

/**
 * Holds the newest decoded picture until the screen's next frame, then
 * draws it: pictures that arrive faster than the screen refreshes replace
 * the one waiting, so the shown picture is never behind the decoder. A
 * still picture stays until the screen changes: the stream's pictures of
 * the same screen behind it are not drawn over it.
 */
class NextFramePresenter extends Disposable {
	private pending: DesktopPicture | undefined;
	private handle: number | undefined;
	/** The newest picture taken is a still. */
	private still = false;

	constructor(
		private readonly targetWindow: Window,
		private readonly present: (picture: DesktopPicture) => void,
		private readonly skip: (picture: DesktopPicture) => void,
	) {
		super();
	}

	public show(picture: DesktopPicture): void {
		if (picture.role === 'unchanged' && this.still) {
			picture.frame.close();
			return;
		}
		if (picture.role !== 'unchanged') {
			this.still = picture.role === 'still';
		}
		if (this.pending) {
			this.pending.frame.close();
			this.skip(this.pending);
		}
		this.pending = picture;
		this.handle ??= this.targetWindow.requestAnimationFrame(() => {
			this.handle = undefined;
			const next = this.pending;
			this.pending = undefined;
			if (next) {
				this.present(next);
			}
		});
	}

	public override dispose(): void {
		if (this.handle !== undefined) {
			this.targetWindow.cancelAnimationFrame(this.handle);
			this.handle = undefined;
		}
		this.pending?.frame.close();
		this.pending = undefined;
		super.dispose();
	}
}

/** Chromium ignores a CSS cursor image larger than this, in CSS pixels. */
const MAX_CSS_CURSOR = 128;

/** A pointer shape drawn at the size the picture is shown at. */
interface PointerLook {
	readonly shape: DesktopPointerShape;
	readonly scale: number;
	readonly ratio: number;
	readonly url: string;
	readonly width: number;
	readonly height: number;
	readonly hotspotX: number;
	readonly hotspotY: number;
}

const codecNames: Readonly<Record<string, string>> = { h264: 'H.264', h265: 'HEVC', vp8: 'VP8', vp9: 'VP9', av1: 'AV1' };

/** Frame rate, codec, bitrate and round trip, then why the quality is lowered, or the bitrate held, when it is. */
export function statsText(stats: DesktopStats, name: string): string {
	const { status } = stats;
	const rate = status.receivedKbps >= 1000
		? nls.localize('desktop.megabits', '{0} Mb/s', (status.receivedKbps / 1000).toFixed(1))
		: nls.localize('desktop.kilobits', '{0} kb/s', status.receivedKbps);
	const parts = [
		nls.localize('desktop.fps', '{0} fps', status.receivedFps),
		codecNames[stats.codec] ?? stats.codec.toUpperCase(),
		rate,
		nls.localize('desktop.rtt', '{0} ms', status.rttMs),
	];
	switch (status.reason) {
		case 'networkPressure': parts.push(nls.localize('desktop.lowNetwork', 'Lowered for the network')); break;
		case 'receiverPressure': parts.push(nls.localize('desktop.lowHere', 'Lowered: this computer is behind')); break;
		case 'encoderPressure': parts.push(nls.localize('desktop.lowThere', 'Lowered: {0} is behind', name)); break;
		case 'recovering': parts.push(nls.localize('desktop.recovering', 'Improving')); break;
		case 'warmingUp': case 'stable': case 'idle': break;
	}
	// A cut already says why; a bitrate that is merely held says what holds it.
	if (status.reason !== 'networkPressure' && status.reason !== 'encoderPressure') {
		switch (status.limit) {
			case 'setting': parts.push(nls.localize('desktop.limitSetting', 'At your bandwidth limit')); break;
			case 'relay': parts.push(nls.localize('desktop.limitRelay', 'Limited by the relay')); break;
			case 'network': parts.push(nls.localize('desktop.limitNetwork', 'Limited by the network')); break;
			case 'encoder': parts.push(nls.localize('desktop.limitEncoder', 'Limited by {0}', name)); break;
			case 'receiver': parts.push(nls.localize('desktop.limitReceiver', 'Limited by this computer')); break;
			case 'none': break;
		}
	}
	return parts.join(' · ');
}

/** One short phrase for what is wrong with the picture, naming whose side it is. */
export function conditionText(condition: DesktopCondition, name: string): string {
	switch (condition) {
		case 'link.blackout': return nls.localize('desktop.condition.blackout', 'No picture from {0}', name);
		case 'relay.backlog': return nls.localize('desktop.condition.relayBacklog', 'Relayed connection is delayed');
		case 'link.congestion': return nls.localize('desktop.condition.congestion', 'Slow network: lowered quality');
		case 'link.loss': return nls.localize('desktop.condition.loss', 'Network is losing data');
		case 'host.secureDesktop': return nls.localize('desktop.condition.secureDesktop', '{0} is on a sign-in or admin prompt', name);
		case 'encoder.slow': return nls.localize('desktop.condition.encoderSlow', '{0} can’t encode fast enough', name);
		case 'decoder.slow': return nls.localize('desktop.condition.decoderSlow', 'This computer can’t decode fast enough');
		case 'decoder.recovering': return nls.localize('desktop.condition.recovering', 'Picture recovering from a decoding error');
		case 'encoder.software': return nls.localize('desktop.condition.encoderSoftware', '{0} is encoding in software', name);
		case 'decoder.software': return nls.localize('desktop.condition.decoderSoftware', 'This computer is decoding in software');
		case 'relay.path': return nls.localize('desktop.condition.relay', 'Connected through a relay');
	}
}

/** Conditions that need no attention while others are absent: a relay is not a problem. */
const informational: ReadonlySet<DesktopCondition> = new Set(['relay.path']);

/** Each measured stage's typical time, then the time nineteen pictures in twenty beat. */
export function latencyText(stats: DesktopStats, name: string): string {
	const labels: Readonly<Record<LatencyStage, string>> = {
		host: nls.localize('desktop.latency.host', 'On {0}', name),
		queue: nls.localize('desktop.latency.queue', 'Waiting here'),
		bridge: nls.localize('desktop.latency.bridge', 'To the window'),
		decode: nls.localize('desktop.latency.decode', 'Decoding'),
		draw: nls.localize('desktop.latency.draw', 'Drawing'),
	};
	const lines = latencyStages.flatMap(stage => {
		const latency = stats.latency[stage];
		return latency ? [nls.localize('desktop.latency.stage', '{0}: {1} ms, {2} ms at most for 95%', labels[stage], latency.p50.toFixed(1), latency.p95.toFixed(1))] : [];
	});
	if (stats.skipped > 0) {
		lines.push(nls.localize('desktop.latency.skipped', '{0} pictures replaced by newer ones before drawing', stats.skipped));
	}
	return lines.length ? [nls.localize('desktop.latency.network', 'Network: about {0} ms each way', Math.round(stats.status.rttMs / 2)), ...lines].join('\n') : '';
}

function problemMessage(name: string, problem: DesktopProblem): string {
	switch (problem) {
		case 'disconnected': return nls.localize('desktop.disconnected', 'Not connected to {0}.', name);
		case 'turnedOff': return nls.localize('desktop.turnedOff', '{0} doesn’t share its screen with this computer.', name);
		case 'needsPermission': return nls.localize('desktop.needsPermission', '{0} hasn’t allowed VniLink to share its screen.', name);
		case 'unsupported': return nls.localize('desktop.unsupported', '{0} can’t share its screen.', name);
		case 'unavailable': return nls.localize('desktop.unavailable', '{0} can’t open its screen right now.', name);
		case 'failed': return nls.localize('desktop.failed', 'The screen of {0} didn’t start.', name);
		case 'linkLost': return nls.localize('desktop.lost', 'The connection to {0} dropped.', name);
		case 'relayUnreachable': return nls.localize('desktop.relayUnreachable', 'The relayed connection to {0} dropped.', name);
		case 'peerClosed': return nls.localize('desktop.peerClosed', '{0} stopped sending its screen.', name);
		case 'hostEncoderFailed': return nls.localize('desktop.hostEncoderFailed', '{0} couldn’t keep encoding its screen.', name);
		case 'hostCaptureFailed': return nls.localize('desktop.hostCaptureFailed', '{0} stopped capturing its screen.', name);
		case 'hostPermission': return nls.localize('desktop.hostPermission', '{0} no longer allows VniLink to record its screen.', name);
		case 'displayGone': return nls.localize('desktop.displayGone', 'The display shown from {0} went away.', name);
		case 'hostStopped': return nls.localize('desktop.hostStopped', '{0} stopped sharing its screen.', name);
		case 'hostShutdown': return nls.localize('desktop.hostShutdown', 'VniLink quit on {0}.', name);
		case 'revoked': return nls.localize('desktop.revoked', '{0} no longer shares its screen with this computer.', name);
		case 'cantShow': return nls.localize('desktop.cantShow', 'This computer can’t show the screen of {0}.', name);
	}
}

/** What the user can do about a problem, when the title alone doesn't say. */
function problemAdvice(name: string, problem: DesktopProblem): string {
	switch (problem) {
		case 'hostPermission': return nls.localize('desktop.hostPermissionAdvice', 'Someone at {0} needs to allow VniLink to record the screen in its system settings. Then try again.', name);
		case 'displayGone': return nls.localize('desktop.displayGoneAdvice', 'Try again to show another display.');
		case 'hostShutdown': return nls.localize('desktop.hostShutdownAdvice', 'Try again once VniLink is open on {0}.', name);
		case 'disconnected': case 'turnedOff': case 'needsPermission': case 'unsupported': case 'unavailable': case 'failed':
		case 'linkLost': case 'relayUnreachable': case 'peerClosed': case 'hostEncoderFailed': case 'hostCaptureFailed':
		case 'hostStopped': case 'revoked': case 'cantShow':
			return '';
	}
}

/** Why a reconnect is under way, as the overlay's body says it. */
function reconnectReason(name: string, problem: DesktopProblem | undefined): string {
	switch (problem) {
		case 'hostEncoderFailed': case 'hostCaptureFailed': case 'peerClosed': return `${problemMessage(name, problem)} ${nls.localize('desktop.reconnectingHost', 'Keys and the pointer stay here until its screen is back.')}`;
		case 'relayUnreachable': return nls.localize('desktop.reconnectingRelay', 'The relayed connection dropped. Keys and the pointer stay here until it’s back.');
		case 'disconnected': case 'turnedOff': case 'needsPermission': case 'unsupported': case 'unavailable': case 'failed':
		case 'linkLost': case 'hostPermission': case 'displayGone': case 'hostStopped': case 'hostShutdown': case 'revoked': case 'cantShow':
		case undefined:
			return nls.localize('desktop.reconnectingBody', 'The connection dropped. Keys and the pointer stay here until it’s back.');
	}
}

/**
 * A paired computer's screen, filling its tab: shown fitted, and, when this
 * computer controls it, the keyboard, pointer and wheel go to it while the
 * screen has focus. One tab per computer. A dropped connection is tried
 * again by itself, over the last picture, dimmed.
 */
export class RemoteDesktopView extends Disposable implements WorkbenchTab {
	public static readonly PREFIX = 'desktop:';
	public readonly id: string;
	public readonly label: string;
	public readonly icon = Icons.remoteDesktop;
	public readonly element: HTMLElement;
	private readonly stage: HTMLElement;
	private readonly canvas: HTMLCanvasElement;
	private readonly context: CanvasRenderingContext2D | null;
	/** The other computer's pointer, drawn over the picture while this computer's pointer isn't standing in for it. */
	private readonly pointerMark: HTMLElement;
	private remotePointer: DesktopPointer | undefined;
	private pointerLook: PointerLook | undefined;
	/** Whether this computer's pointer is over the picture. */
	private pointerInside = false;
	private readonly overlay: HTMLElement;
	private readonly overlayTitle: HTMLElement;
	private readonly overlayBody: HTMLElement;
	private readonly overlayActions: HTMLElement;
	private readonly statusRole: HTMLElement;
	private readonly statusSize: HTMLElement;
	private readonly statusStats: HTMLElement;
	/** What is wrong with the live picture, apart from why one ended. */
	private readonly statusCondition: HTMLElement;
	/** The condition last shown and announced. */
	private shownCondition: DesktopCondition | undefined;
	private readonly statsTimer = this._register(new MutableDisposable());
	private readonly session = this._register(new MutableDisposable<IDesktopSession>());
	private readonly sessionListeners = this._register(new DisposableStore());
	private readonly overlayListeners = this._register(new DisposableStore());
	private readonly retryTimer = this._register(new MutableDisposable());
	private retries = 0;
	private attempt = 0;
	/** Whether this tab already reconnected once after a picture it couldn't show. */
	private retriedCodec = false;
	private hasPicture = false;
	/** The end of the running picture that the current reconnect answers. */
	private reconnectingFor: DesktopProblem | undefined;
	/** The display shown, and its bounds, as the other computer reports them at the picture's revision. */
	private shown: { readonly display: string; readonly revision: string } | undefined;
	/** The shown display's bounds, once the other computer's topology names it at the picture's revision. */
	private display: DisplayInfo | undefined;
	private readingDisplays = false;
	private lastRead = -Infinity;
	/** The display this session's pointer is selected for, sent once per session before any pointer. */
	private selected: string | undefined;
	private wheel = { dx: 0, dy: 0 };
	private pressed = false;
	/** Keys sent down and not yet up, by usage. */
	private readonly down = new Set<number>();
	/**
	 * On Windows, AltGr arrives as a left Control press just before it. That
	 * Control is held back until the next key says which it was, and dropped
	 * with its release when it was AltGr's.
	 */
	private heldControl = false;
	private phantomControl = false;

	constructor(
		private readonly device: DeviceSummary,
		/** Whether this computer controls the other one, or only views its screen. */
		public readonly control: boolean,
		@IRemoteDesktopService private readonly remoteDesktop: IRemoteDesktopService,
		@ITabService private readonly tabService: ITabService,
	) {
		super();
		this.id = RemoteDesktopView.idFor(device.id);
		this.label = device.name;
		this.element = $('section.desktop-view');
		this.element.setAttribute('role', 'region');
		this.element.setAttribute('aria-label', control
			? nls.localize('desktop.regionControl', 'Controlling {0}', device.name)
			: nls.localize('desktop.regionView', 'Viewing {0}', device.name));
		this.stage = append(this.element, $('.desktop-stage'));
		this.canvas = append(this.stage, $<HTMLCanvasElement>('canvas.desktop-screen'));
		this.canvas.tabIndex = 0;
		this.canvas.setAttribute('role', 'application');
		this.canvas.setAttribute('aria-roledescription', nls.localize('desktop.screenRole', 'remote screen'));
		this.canvas.setAttribute('aria-label', control
			? nls.localize('desktop.screenControl', 'The screen of {0}. While it has focus, keys and the pointer go to {0}.', device.name)
			: nls.localize('desktop.screenView', 'The screen of {0}, to view only.', device.name));
		this.context = this.canvas.getContext('2d');
		this.pointerMark = append(this.stage, $('.desktop-pointer', { 'aria-hidden': 'true' }));
		this.pointerMark.hidden = true;
		const resize = this._register(new DisposableResizeObserver('remoteDesktopPointer', () => {
			this.showPointer();
		}, getWindow(this.element)));
		this._register(resize.observe(this.canvas));
		this._register(addDisposableListener(this.canvas, EventType.MOUSE_ENTER, () => {
			this.pointerInside = true;
			this.showPointer();
		}));
		this._register(addDisposableListener(this.canvas, EventType.MOUSE_LEAVE, () => {
			this.pointerInside = false;
			this.showPointer();
		}));
		this.overlay = append(this.stage, $('.desktop-overlay', { role: 'status' }));
		const panel = append(this.overlay, $('.desktop-overlay-panel'));
		append(panel, $('span.desktop-spinner', { 'aria-hidden': 'true' }));
		this.overlayTitle = append(panel, $('p.desktop-overlay-title'));
		this.overlayBody = append(panel, $('p.desktop-overlay-body'));
		this.overlayActions = append(panel, $('.desktop-overlay-actions'));
		const status = append(this.element, $('footer.desktop-status'));
		this.statusRole = append(status, $('span.desktop-status-role'));
		this.statusStats = append(status, $('span.desktop-status-stats'));
		this.statusCondition = append(status, $('span.desktop-status-condition'));
		this.statusCondition.hidden = true;
		append(status, $('span.desktop-status-space'));
		this.statusSize = append(status, $('span.desktop-status-size'));
		if (control) {
			this.registerInput();
		}
		this._register(this.tabService.onDidChange(() => {
			if (this.tabService.activeTab !== this) {
				this.releaseInput();
			}
		}));
		this._register(toDisposable(() => {
			this.releaseInput();
		}));
		void this.connect();
	}

	public static idFor(deviceId: string): string {
		return `${RemoteDesktopView.PREFIX}${deviceId}`;
	}

	/** Opens the screen, or says in the tab why it can't; `again` after the connection dropped. */
	private async connect(again = false): Promise<void> {
		const attempt = ++this.attempt;
		this.retryTimer.clear();
		if (!again) {
			this.retries = 0;
		}
		this.showState(again ? 'reconnecting' : 'connecting');
		const result = await this.remoteDesktop.open(this.device.id, this.control);
		if (attempt !== this.attempt || this._store.isDisposed) {
			if ('session' in result) {
				result.session.dispose();
			}
			return;
		}
		if ('failure' in result) {
			this.showProblem(result.failure, again);
			return;
		}
		this.sessionListeners.clear();
		const session = result.session;
		const presenter = this.sessionListeners.add(new NextFramePresenter(getWindow(this.element), picture => {
			this.draw(picture);
		}, picture => {
			session.skipped(picture);
		}));
		this.sessionListeners.add(session.onPicture(picture => {
			presenter.show(picture);
		}));
		this.sessionListeners.add(session.onPointer(pointer => {
			this.remotePointer = pointer;
			this.showPointer();
		}));
		this.sessionListeners.add(session.onEnd(end => {
			this.releaseInput();
			this.showProblem(end, true);
		}));
		this.session.value = session;
		this.remotePointer = undefined;
		this.showPointer();
		this.selected = undefined;
		this.display = undefined;
		this.shown = undefined;
		this.lastRead = -Infinity;
		this.retries = 0;
	}

	/** Tries again after 1, 2, 5 and 10 seconds, then every 30, while the tab is open. */
	private scheduleRetry(): void {
		const delays = [1000, 2000, 5000, 10000];
		const delay = delays.at(this.retries) ?? 30000;
		this.retries++;
		const timer = setTimeout(() => {
			void this.connect(true);
		}, delay);
		this.retryTimer.value = toDisposable(() => {
			clearTimeout(timer);
		});
	}

	private draw(picture: DesktopPicture): void {
		const { frame, display, revision } = picture;
		try {
			if (!this.hasPicture || this.overlay.dataset.state) {
				this.hasPicture = true;
				this.hideState();
			}
			if (this.shown?.display !== display || this.shown.revision !== revision) {
				this.shown = { display, revision };
				this.display = undefined;
			}
			if (this.control && this.selected !== display) {
				this.selected = display;
				this.session.value?.send({ type: 'selectDisplays', ids: [display] });
			}
			if (!this.display) {
				void this.readDisplays();
			}
			// A background tab keeps decoding, so showing it again is immediate, but draws nothing.
			if (!this.context || this.tabService.activeTab !== this) {
				return;
			}
			if (this.canvas.width !== frame.displayWidth || this.canvas.height !== frame.displayHeight) {
				this.canvas.width = frame.displayWidth;
				this.canvas.height = frame.displayHeight;
				this.showPointer();
			}
			this.context.drawImage(frame, 0, 0);
			this.session.value?.presented(picture);
		} finally {
			frame.close();
		}
	}

	/**
	 * The shown display's bounds from the other computer's topology, which
	 * may arrive just after the first picture: asked again, at most twice a
	 * second, until it names the display at the picture's revision.
	 */
	private async readDisplays(): Promise<void> {
		const session = this.session.value;
		const shown = this.shown;
		if (!session || !shown || this.readingDisplays || performance.now() - this.lastRead < 500) {
			return;
		}
		this.readingDisplays = true;
		this.lastRead = performance.now();
		try {
			const found = (await session.displays()).find(display => display.id === shown.display && display.revision === shown.revision);
			if (found && this.shown === shown) {
				this.display = found;
			}
		} catch {
			// The next picture asks again.
		} finally {
			this.readingDisplays = false;
		}
		this.updateStatus();
	}

	private currentDisplay(): DisplayInfo | undefined {
		return this.display;
	}

	private updateStatus(): void {
		const display = this.currentDisplay();
		this.statusSize.textContent = display ? nls.localize('desktop.size', '{0} × {1}', display.width, display.height) : '';
	}

	private registerInput(): void {
		const send = (input: DesktopInput): void => {
			this.session.value?.send(input);
		};
		this._register(addDisposableListener(this.canvas, EventType.KEY_DOWN, event => {
			this.key(event, true);
		}));
		this._register(addDisposableListener(this.canvas, EventType.KEY_UP, event => {
			this.key(event, false);
		}));
		this._register(addDisposableListener(this.canvas, EventType.POINTER_MOVE, event => {
			this.pointer(event);
		}));
		this._register(addDisposableListener(this.canvas, EventType.POINTER_DOWN, event => {
			const button = buttons.at(event.button);
			if (!button) {
				return;
			}
			event.preventDefault();
			this.flushControl();
			this.canvas.focus();
			try {
				// A drag that leaves the picture keeps going to the other computer until the button is up.
				this.canvas.setPointerCapture(event.pointerId);
			} catch {
				// A pointer the browser no longer tracks cannot be captured.
			}
			this.pointer(event);
			this.pressed = true;
			send({ type: 'submitButton', button, down: true });
		}));
		this._register(addDisposableListener(this.canvas, EventType.POINTER_UP, event => {
			const button = buttons.at(event.button);
			if (button) {
				event.preventDefault();
				this.pointer(event);
				send({ type: 'submitButton', button, down: false });
			}
		}));
		this._register(addDisposableListener(this.canvas, EventType.CONTEXT_MENU, event => {
			event.preventDefault();
		}));
		this._register(addDisposableListener(this.canvas, EventType.MOUSE_WHEEL, (event: WheelEvent) => {
			event.preventDefault();
			this.scroll(event);
		}, { passive: false }));
		this._register(addDisposableListener(this.canvas, EventType.BLUR, () => {
			this.releaseInput();
		}));
	}

	/**
	 * A key, by where it is on the keyboard, with the character it typed:
	 * the other computer types that character whatever its own layout.
	 * Shortcuts stay keys: no character travels with Control or Command, nor
	 * on Windows with Alt, whose shortcuts type nothing. AltGr types, and so
	 * does Option on a Mac, as both make characters there.
	 */
	private key(event: KeyboardEvent, down: boolean): void {
		const usage = hidUsage(event.code);
		if (usage === undefined || !this.session.value) {
			return;
		}
		event.preventDefault();
		event.stopPropagation();
		if (isWindows) {
			if (down && event.code === 'ControlLeft' && !event.repeat && !this.down.has(CONTROL_LEFT)) {
				this.heldControl = true;
				this.pressed = true;
				return;
			}
			if (this.heldControl) {
				this.heldControl = false;
				if (down && event.code === 'AltRight' && event.key === 'AltGraph') {
					this.phantomControl = true;
				} else {
					this.sendKey(CONTROL_LEFT, true, '', false);
				}
			}
			if (!down && event.code === 'ControlLeft' && this.phantomControl) {
				this.phantomControl = false;
				return;
			}
		}
		// Windows reports Print Screen only as it is let go.
		if (!down && !this.down.has(usage)) {
			this.sendKey(usage, true, '', false);
		}
		const altGr = event.getModifierState('AltGraph');
		const shortcut = event.metaKey || (event.ctrlKey && !altGr) || (!isMacintosh && event.altKey && !altGr);
		// One character, emoji included; named keys such as Enter are longer.
		const point = event.key.codePointAt(0);
		const character = point !== undefined && String.fromCodePoint(point) === event.key;
		const composing = down && (event.key === 'Dead' || event.key === 'Process' || event.isComposing);
		// A held key repeats as the other computer types it: typed characters don't repeat there by themselves.
		this.sendKey(usage, down, down && character && !shortcut ? event.key : '', composing);
	}

	private sendKey(usage: number, down: boolean, text: string, composing: boolean): void {
		if (down) {
			this.down.add(usage);
		} else {
			this.down.delete(usage);
		}
		this.pressed = true;
		this.session.value?.send({ type: 'submitLocalKey', usage, down, text, composing });
	}

	/** A Control held back for AltGr goes down before anything that isn't a key, such as Control-click. */
	private flushControl(): void {
		if (this.heldControl) {
			this.heldControl = false;
			this.sendKey(CONTROL_LEFT, true, '', false);
		}
	}

	/**
	 * Where the picture is drawn inside the canvas, which fits it with
	 * margins: its top-left corner in the viewport, and CSS pixels per
	 * picture pixel.
	 */
	private fitted(): { readonly left: number; readonly top: number; readonly scale: number } | undefined {
		if (!this.canvas.width || !this.canvas.height) {
			return undefined;
		}
		const box = this.canvas.getBoundingClientRect();
		const scale = Math.min(box.width / this.canvas.width, box.height / this.canvas.height);
		if (!(scale > 0)) {
			return undefined;
		}
		return {
			left: box.left + (box.width - this.canvas.width * scale) / 2,
			top: box.top + (box.height - this.canvas.height * scale) / 2,
			scale,
		};
	}

	/**
	 * Shows the other computer's pointer, when it reports it apart from the
	 * picture. While this computer controls it and its own pointer is over
	 * the picture, that pointer takes the other one's shape, so it moves
	 * without waiting for the network; otherwise the other pointer is drawn
	 * where it is.
	 */
	private showPointer(): void {
		const remote = this.remotePointer;
		const fit = this.fitted();
		const look = remote?.shape && fit && this.hasPicture ? this.lookFor(remote.shape, fit.scale) : undefined;
		if (!remote || !fit || !look) {
			this.canvas.style.cursor = '';
			this.pointerMark.hidden = true;
			return;
		}
		if (this.control) {
			this.canvas.style.cursor = remote.visible
				? `image-set(url("${look.url}") ${String(look.ratio)}x) ${String(look.hotspotX)} ${String(look.hotspotY)}, default`
				: 'none';
		}
		const drawn = remote.visible && (!this.control || !this.pointerInside);
		this.pointerMark.hidden = !drawn;
		if (drawn) {
			const stage = this.stage.getBoundingClientRect();
			const x = fit.left - stage.left + remote.x * fit.scale - look.hotspotX;
			const y = fit.top - stage.top + remote.y * fit.scale - look.hotspotY;
			this.pointerMark.style.width = `${String(look.width)}px`;
			this.pointerMark.style.height = `${String(look.height)}px`;
			this.pointerMark.style.backgroundImage = `url("${look.url}")`;
			this.pointerMark.style.transform = `translate(${String(x)}px, ${String(y)}px)`;
		}
	}

	/** The shape as an image at the picture's shown scale and this screen's density, made once per change. */
	private lookFor(shape: DesktopPointerShape, scale: number): PointerLook | undefined {
		const ratio = getWindow(this.element).devicePixelRatio || 1;
		const previous = this.pointerLook;
		if (previous?.shape === shape && previous.scale === scale && previous.ratio === ratio) {
			return previous;
		}
		const fit = Math.min(scale, MAX_CSS_CURSOR / shape.width, MAX_CSS_CURSOR / shape.height);
		const width = Math.max(1, Math.round(shape.width * fit));
		const height = Math.max(1, Math.round(shape.height * fit));
		const source = $<HTMLCanvasElement>('canvas');
		source.width = shape.width;
		source.height = shape.height;
		const target = $<HTMLCanvasElement>('canvas');
		target.width = Math.max(1, Math.round(width * ratio));
		target.height = Math.max(1, Math.round(height * ratio));
		const sourceContext = source.getContext('2d');
		const targetContext = target.getContext('2d');
		if (!sourceContext || !targetContext) {
			return undefined;
		}
		sourceContext.putImageData(new ImageData(shape.rgba, shape.width, shape.height), 0, 0);
		targetContext.imageSmoothingQuality = 'high';
		targetContext.drawImage(source, 0, 0, target.width, target.height);
		this.pointerLook = {
			shape,
			scale,
			ratio,
			url: target.toDataURL('image/png'),
			width,
			height,
			hotspotX: Math.min(width - 1, Math.round(shape.hotspotX * fit)),
			hotspotY: Math.min(height - 1, Math.round(shape.hotspotY * fit)),
		};
		return this.pointerLook;
	}

	/** Where the pointer is on the other computer's display, through the picture as it is fitted on screen. */
	private pointer(event: PointerEvent): void {
		const display = this.currentDisplay();
		const shown = this.shown;
		const fit = this.fitted();
		if (!display || !shown || !fit) {
			return;
		}
		const u = (event.clientX - fit.left) / (this.canvas.width * fit.scale);
		const v = (event.clientY - fit.top) / (this.canvas.height * fit.scale);
		if (u < 0 || u > 1 || v < 0 || v > 1) {
			return;
		}
		this.session.value?.send({
			type: 'submitPointer',
			x: display.origin_x + Math.min(display.width - 1, Math.floor(u * display.width)),
			y: display.origin_y + Math.min(display.height - 1, Math.floor(v * display.height)),
			display: shown.display,
			revision: shown.revision,
		});
	}

	/** Scrolling, in the engine's precise units; what is left of a unit waits for the next event. */
	private scroll(event: WheelEvent): void {
		const factor = event.deltaMode === WheelEvent.DOM_DELTA_LINE
			? WHEEL_NOTCH / 3
			: event.deltaMode === WheelEvent.DOM_DELTA_PAGE ? WHEEL_NOTCH * 3 : WHEEL_PER_PIXEL;
		this.wheel = { dx: this.wheel.dx + event.deltaX * factor, dy: this.wheel.dy + event.deltaY * factor };
		const dx = Math.max(-WHEEL_LIMIT, Math.min(WHEEL_LIMIT, Math.trunc(this.wheel.dx)));
		const dy = Math.max(-WHEEL_LIMIT, Math.min(WHEEL_LIMIT, Math.trunc(this.wheel.dy)));
		if (dx || dy) {
			this.wheel = { dx: this.wheel.dx - dx, dy: this.wheel.dy - dy };
			this.session.value?.send({ type: 'submitPreciseWheel', dx, dy });
		}
	}

	/** Lets go of every key and button held on the other computer. */
	private releaseInput(): void {
		this.heldControl = false;
		this.phantomControl = false;
		this.down.clear();
		if (this.pressed) {
			this.pressed = false;
			this.session.value?.send({ type: 'releaseSessionInput' });
		}
	}

	/** `why` is the problem a reconnect answers, when one does. */
	private showState(state: 'connecting' | 'reconnecting', why?: DesktopProblem): void {
		const name = this.device.name;
		if (why) {
			this.reconnectingFor = why;
		} else if (state === 'connecting') {
			this.reconnectingFor = undefined;
		}
		this.setOverlay(
			state,
			state === 'connecting' ? nls.localize('desktop.connecting', 'Connecting to {0}…', name) : nls.localize('desktop.reconnecting', 'Reconnecting to {0}…', name),
			state === 'reconnecting' ? reconnectReason(name, this.reconnectingFor) : '',
			[{ label: state === 'connecting' ? nls.localize('desktop.cancel', 'Cancel') : nls.localize('desktop.disconnect', 'Disconnect'), run: () => void this.tabService.close(this.id) }],
		);
	}

	private showProblem(problem: DesktopProblem, again: boolean): void {
		const name = this.device.name;
		// The next offer leaves out the codec this computer couldn't show:
		// try once more by itself before saying so.
		if (problem === 'cantShow' && !this.retriedCodec) {
			this.retriedCodec = true;
			void this.connect(true);
			return;
		}
		if (retryable.has(problem) && (again || reconnectAtOnce.has(problem))) {
			this.showState('reconnecting', reconnectAtOnce.has(problem) ? problem : undefined);
			this.scheduleRetry();
			return;
		}
		this.setOverlay('ended', problemMessage(name, problem), problemAdvice(name, problem), [
			{ label: nls.localize('desktop.tryAgain', 'Try Again'), primary: true, run: () => void this.connect() },
			{ label: nls.localize('desktop.closeTab', 'Close Tab'), run: () => void this.tabService.close(this.id) },
		]);
	}

	private setOverlay(state: string, title: string, body: string, actions: readonly OverlayAction[]): void {
		this.statsTimer.clear();
		this.statusStats.textContent = '';
		this.showConditions([]);
		// Progress is polite; an end interrupts, once, from the same region.
		this.overlay.setAttribute('role', state === 'ended' ? 'alert' : 'status');
		this.overlay.dataset.state = state;
		this.overlay.hidden = false;
		this.stage.classList.toggle('desktop-dimmed', this.hasPicture);
		this.overlayTitle.textContent = title;
		this.overlayBody.textContent = body;
		this.overlayBody.hidden = !body;
		this.overlayListeners.clear();
		this.overlayActions.replaceChildren();
		for (const action of actions) {
			const button = append(this.overlayActions, $<HTMLButtonElement>('button.desktop-overlay-button', { type: 'button' }));
			button.textContent = action.label;
			button.classList.toggle('primary', action.primary === true);
			this.overlayListeners.add(addDisposableListener(button, EventType.CLICK, action.run));
		}
		this.statusRole.textContent = state === 'ended'
			? nls.localize('desktop.notConnected', 'Not connected')
			: state === 'connecting' ? nls.localize('desktop.statusConnecting', 'Connecting') : nls.localize('desktop.statusReconnecting', 'Reconnecting');
		this.statusRole.dataset.state = state;
	}

	/** Reads how the picture is doing once a second while it shows. */
	private watchStats(): void {
		const timer = setInterval(() => {
			void this.session.value?.stats().then(stats => {
				// The picture may have ended while the engine answered.
				if (this.overlay.dataset.state) {
					return;
				}
				this.statusStats.textContent = stats ? statsText(stats, this.device.name) : '';
				this.statusStats.title = stats ? latencyText(stats, this.device.name) : '';
				this.showConditions(stats?.status.conditions ?? []);
			});
		}, 1000);
		this.statsTimer.value = toDisposable(() => {
			clearInterval(timer);
		});
	}

	/**
	 * Shows the worst condition that needs attention in one phrase, with
	 * every held one on hover or focus, and says a new worst one aloud
	 * once. A relay alone is not shown; it is listed with the others.
	 */
	private showConditions(conditions: readonly DesktopCondition[]): void {
		const name = this.device.name;
		const worst = conditions.find(condition => !informational.has(condition));
		const element = this.statusCondition;
		element.hidden = worst === undefined;
		element.textContent = worst ? conditionText(worst, name) : '';
		const all = conditions.map(condition => conditionText(condition, name)).join('\n');
		element.title = all;
		element.tabIndex = worst ? 0 : -1;
		element.setAttribute('aria-label', conditions.map(condition => conditionText(condition, name)).join('. '));
		if (worst) {
			element.dataset.condition = worst;
		} else {
			delete element.dataset.condition;
		}
		if (worst !== this.shownCondition) {
			this.shownCondition = worst;
			if (worst) {
				announce(conditionText(worst, name));
			}
		}
	}

	private hideState(): void {
		this.watchStats();
		delete this.overlay.dataset.state;
		this.overlay.hidden = true;
		this.overlayListeners.clear();
		this.stage.classList.remove('desktop-dimmed');
		this.statusRole.textContent = this.control
			? nls.localize('desktop.controlling', 'Controlling {0}', this.device.name)
			: nls.localize('desktop.viewing', 'Viewing {0}', this.device.name);
		this.statusRole.dataset.state = 'live';
		if (this.control && !this.element.contains(document.activeElement) && this.tabService.activeTab === this) {
			this.canvas.focus();
		}
	}
}
