import * as nls from '../../../nls.js';
import { $, addDisposableListener, append, EventType } from '../../../base/browser/dom.js';
import { Icons } from '../../../base/common/icons.js';
import { isMacintosh, isWindows } from '../../../base/common/platform.js';
import { Disposable, DisposableStore, MutableDisposable, toDisposable } from '../../../base/common/lifecycle.js';
import type { DisplayInfo } from '../../../platform/engine/common/engine.js';
import { hidUsage } from '../../../platform/engine/common/keyUsage.js';
import type { DeviceSummary } from '../../services/devices/deviceService.js';
import { IRemoteDesktopService, type DesktopEnd, type DesktopFailure, type DesktopInput, type DesktopPicture, type IDesktopSession } from '../../services/remoteDesktop/remoteDesktopService.js';
import { ITabService, type WorkbenchTab } from '../../services/tabs/tabService.js';

type DesktopProblem = DesktopFailure | DesktopEnd;

/** Problems another attempt may fix, tried again by themselves; the rest need a change on either computer. */
const retryable: ReadonlySet<DesktopProblem> = new Set(['disconnected', 'unavailable', 'failed', 'lost']);

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

function problemMessage(name: string, problem: DesktopProblem): string {
	switch (problem) {
		case 'disconnected': return nls.localize('desktop.disconnected', 'Not connected to {0}.', name);
		case 'turnedOff': return nls.localize('desktop.turnedOff', '{0} doesn’t share its screen with this computer.', name);
		case 'needsPermission': return nls.localize('desktop.needsPermission', '{0} hasn’t allowed VniLink to share its screen.', name);
		case 'unsupported': return nls.localize('desktop.unsupported', '{0} can’t share its screen.', name);
		case 'unavailable': return nls.localize('desktop.unavailable', '{0} can’t open its screen right now.', name);
		case 'failed': return nls.localize('desktop.failed', 'The screen of {0} didn’t start.', name);
		case 'lost': return nls.localize('desktop.lost', 'The connection to {0} dropped.', name);
		case 'cantShow': return nls.localize('desktop.cantShow', 'This computer can’t show the screen of {0}.', name);
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
	private readonly overlay: HTMLElement;
	private readonly overlayTitle: HTMLElement;
	private readonly overlayBody: HTMLElement;
	private readonly overlayActions: HTMLElement;
	private readonly statusRole: HTMLElement;
	private readonly statusSize: HTMLElement;
	private readonly session = this._register(new MutableDisposable<IDesktopSession>());
	private readonly sessionListeners = this._register(new DisposableStore());
	private readonly overlayListeners = this._register(new DisposableStore());
	private readonly retryTimer = this._register(new MutableDisposable());
	private retries = 0;
	private attempt = 0;
	private hasPicture = false;
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
		this.overlay = append(this.stage, $('.desktop-overlay', { role: 'status' }));
		const panel = append(this.overlay, $('.desktop-overlay-panel'));
		append(panel, $('span.desktop-spinner', { 'aria-hidden': 'true' }));
		this.overlayTitle = append(panel, $('p.desktop-overlay-title'));
		this.overlayBody = append(panel, $('p.desktop-overlay-body'));
		this.overlayActions = append(panel, $('.desktop-overlay-actions'));
		const status = append(this.element, $('footer.desktop-status'));
		this.statusRole = append(status, $('span.desktop-status-role'));
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
		this.sessionListeners.add(session.onPicture(picture => {
			this.draw(picture);
		}));
		this.sessionListeners.add(session.onEnd(end => {
			this.releaseInput();
			this.showProblem(end, true);
		}));
		this.session.value = session;
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

	private draw({ frame, display, revision }: DesktopPicture): void {
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
			}
			this.context.drawImage(frame, 0, 0);
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

	/** Where the pointer is on the other computer's display, through the picture as it is fitted on screen. */
	private pointer(event: PointerEvent): void {
		const display = this.currentDisplay();
		const shown = this.shown;
		if (!display || !shown || !this.canvas.width || !this.canvas.height) {
			return;
		}
		const box = this.canvas.getBoundingClientRect();
		const scale = Math.min(box.width / this.canvas.width, box.height / this.canvas.height);
		const width = this.canvas.width * scale;
		const height = this.canvas.height * scale;
		const u = (event.clientX - box.left - (box.width - width) / 2) / width;
		const v = (event.clientY - box.top - (box.height - height) / 2) / height;
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

	private showState(state: 'connecting' | 'reconnecting'): void {
		const name = this.device.name;
		this.setOverlay(
			state,
			state === 'connecting' ? nls.localize('desktop.connecting', 'Connecting to {0}…', name) : nls.localize('desktop.reconnecting', 'Reconnecting to {0}…', name),
			state === 'reconnecting' ? nls.localize('desktop.reconnectingBody', 'The connection dropped. Keys and the pointer stay here until it’s back.') : '',
			[{ label: state === 'connecting' ? nls.localize('desktop.cancel', 'Cancel') : nls.localize('desktop.disconnect', 'Disconnect'), run: () => void this.tabService.close(this.id) }],
		);
	}

	private showProblem(problem: DesktopProblem, again: boolean): void {
		const name = this.device.name;
		if (retryable.has(problem) && (again || problem === 'lost')) {
			this.showState('reconnecting');
			this.scheduleRetry();
			return;
		}
		this.setOverlay('ended', problemMessage(name, problem), '', [
			{ label: nls.localize('desktop.tryAgain', 'Try Again'), primary: true, run: () => void this.connect() },
			{ label: nls.localize('desktop.closeTab', 'Close Tab'), run: () => void this.tabService.close(this.id) },
		]);
	}

	private setOverlay(state: string, title: string, body: string, actions: readonly OverlayAction[]): void {
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

	private hideState(): void {
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
