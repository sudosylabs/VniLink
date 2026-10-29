import * as nls from '../../../nls.js';
import { $, addDisposableListener, append, EventType, isAncestor } from '../../../base/browser/dom.js';
import { renderIcon } from '../../../base/browser/ui/icon/icon.js';
import type { IAction } from '../../../base/common/actions.js';
import { Emitter } from '../../../base/common/event.js';
import { Icons, type IconId } from '../../../base/common/icons.js';
import { Disposable, MutableDisposable, toDisposable } from '../../../base/common/lifecycle.js';
import { IContextMenuService } from '../../../platform/contextview/browser/contextView.js';
import type { DesktopPhase } from './desktopStatsOverlay.js';

/** How close to the top edge the pointer brings the bar down, in CSS pixels. */
const revealEdge = 6;

/** How long the bar stays after the pointer leaves it. */
const hideDelay = 1200;

/** What the bar shows; the view that owns the session decides it. */
export interface DesktopBarModel {
	readonly phase: DesktopPhase;
	/** The picture size chosen, as its menu names it. */
	readonly size: string;
	/** The display shown, as its menu names it; `undefined` hides the menu, as with one display or none to choose. */
	readonly display: string | undefined;
	/** The other computer's sound here; `undefined` while it has none to play. */
	readonly sound: 'playing' | 'muted' | undefined;
	/** Privacy mode on the other computer; `undefined` where it can't be asked for. */
	readonly privacy: boolean | undefined;
	/** The running recording's length; `undefined` while none runs. */
	readonly recording: string | undefined;
	readonly fullScreen: boolean;
}

/** What the bar's controls ask of the view. */
export interface DesktopBarDelegate {
	sizeActions(): IAction[];
	displayActions(): IAction[];
	moreActions(): IAction[];
	toggleSound(): void;
	togglePrivacy(): void;
	stopRecording(): void;
	toggleFullScreen(): void;
}

/**
 * A remote desktop's controls, kept off the picture until wanted: the
 * pointer at the top edge, or the small tab there, brings them down. They
 * stay while a menu is open or they have focus, and leave a moment after
 * the pointer does.
 */
export class DesktopSessionBar extends Disposable {
	public readonly element: HTMLElement;
	private readonly tab: HTMLButtonElement;
	private readonly bar: HTMLElement;
	private readonly state: HTMLElement;
	private readonly sizeButton: HTMLButtonElement;
	private readonly displayButton: HTMLButtonElement;
	private readonly soundButton: HTMLButtonElement;
	private readonly privacyButton: HTMLButtonElement;
	private readonly recordingButton: HTMLButtonElement;
	private readonly moreButton: HTMLButtonElement;
	private readonly fullScreenButton: HTMLButtonElement;
	private readonly hideTimer = this._register(new MutableDisposable());
	private readonly left = this._register(new Emitter<void>());
	/** The bar went away holding focus, which the view gives back to the screen. */
	public readonly onDidLeave = this.left.event;
	private menuOpen = false;

	constructor(
		private readonly host: HTMLElement,
		name: string,
		private readonly delegate: DesktopBarDelegate,
		@IContextMenuService private readonly contextMenuService: IContextMenuService,
	) {
		super();
		this.element = $('.desktop-bar-host');
		this.tab = append(this.element, $<HTMLButtonElement>('button.desktop-bar-tab', { type: 'button' }));
		this.tab.setAttribute('aria-label', nls.localize('desktop.showControls', 'Show controls for {0}', name));
		this.tab.title = nls.localize('desktop.showControlsHint', 'Controls: point at the top edge');
		this.tab.append(renderIcon(Icons.chevronDown));
		this.bar = append(this.element, $('.desktop-bar', { role: 'toolbar', 'aria-label': nls.localize('desktop.controls', '{0} session', name) }));
		this.state = append(this.bar, $('span.desktop-bar-name'));
		this.state.textContent = name;
		append(this.bar, $('span.desktop-bar-divider'));
		this.sizeButton = this.button('desktop-bar-menu', Icons.fitToView);
		this.sizeButton.setAttribute('aria-haspopup', 'menu');
		this.displayButton = this.button('desktop-bar-menu', Icons.remoteDesktop);
		this.displayButton.setAttribute('aria-haspopup', 'menu');
		append(this.bar, $('span.desktop-bar-divider'));
		this.recordingButton = this.button('desktop-bar-recording');
		this.soundButton = this.button('desktop-bar-icon');
		this.privacyButton = this.button('desktop-bar-privacy');
		this.moreButton = this.button('desktop-bar-icon', Icons.more);
		this.moreButton.setAttribute('aria-haspopup', 'menu');
		describe(this.moreButton, nls.localize('desktop.more', 'More'));
		append(this.bar, $('span.desktop-bar-divider'));
		this.fullScreenButton = this.button('desktop-bar-icon');
		this.registerListeners();
	}

	private button(className: string, icon?: IconId): HTMLButtonElement {
		const button = append(this.bar, $<HTMLButtonElement>(`button.${className}`, { type: 'button' }));
		if (icon) {
			button.append(renderIcon(icon));
		}
		return button;
	}

	private registerListeners(): void {
		this._register(addDisposableListener(this.host, EventType.POINTER_MOVE, event => {
			if (event.clientY - this.host.getBoundingClientRect().top <= revealEdge) {
				this.reveal();
			}
		}));
		this._register(addDisposableListener(this.bar, 'pointerenter', () => {
			this.reveal();
		}));
		this._register(addDisposableListener(this.bar, 'pointerleave', () => {
			this.hideSoon();
		}));
		this._register(addDisposableListener(this.bar, EventType.FOCUS_OUT, () => {
			this.hideSoon();
		}));
		this._register(addDisposableListener(this.tab, EventType.CLICK, () => {
			this.reveal();
			this.sizeButton.focus();
		}));
		this._register(addDisposableListener(this.bar, EventType.KEY_DOWN, event => {
			if (event.key === 'Escape') {
				event.preventDefault();
				this.hide();
				this.tab.focus();
			}
		}));
		this._register(addDisposableListener(this.sizeButton, EventType.CLICK, () => {
			this.showMenu(this.sizeButton, this.delegate.sizeActions());
		}));
		this._register(addDisposableListener(this.displayButton, EventType.CLICK, () => {
			this.showMenu(this.displayButton, this.delegate.displayActions());
		}));
		this._register(addDisposableListener(this.moreButton, EventType.CLICK, () => {
			this.showMenu(this.moreButton, this.delegate.moreActions());
		}));
		this._register(addDisposableListener(this.soundButton, EventType.CLICK, () => {
			this.delegate.toggleSound();
		}));
		this._register(addDisposableListener(this.privacyButton, EventType.CLICK, () => {
			this.delegate.togglePrivacy();
		}));
		this._register(addDisposableListener(this.recordingButton, EventType.CLICK, () => {
			this.delegate.stopRecording();
		}));
		this._register(addDisposableListener(this.fullScreenButton, EventType.CLICK, () => {
			this.delegate.toggleFullScreen();
		}));
	}

	private showMenu(anchor: HTMLElement, actions: IAction[]): void {
		this.menuOpen = true;
		this.reveal();
		anchor.setAttribute('aria-expanded', 'true');
		this.contextMenuService.showContextMenu({
			getAnchor: () => anchor,
			getActions: () => actions,
			onHide: () => {
				this.menuOpen = false;
				anchor.setAttribute('aria-expanded', 'false');
				this.hideSoon();
			},
		});
	}

	public get revealed(): boolean {
		return this.element.classList.contains('revealed');
	}

	public reveal(): void {
		this.hideTimer.clear();
		this.element.classList.add('revealed');
	}

	private hideSoon(): void {
		const timer = setTimeout(() => {
			this.hide();
		}, hideDelay);
		this.hideTimer.value = toDisposable(() => {
			clearTimeout(timer);
		});
	}

	private hide(): void {
		this.hideTimer.clear();
		// Kept while it is in use: a menu open from it, or keyboard focus in it.
		if (this.menuOpen || this.bar.querySelector(':focus-visible')) {
			return;
		}
		const wasRevealed = this.revealed;
		this.element.classList.remove('revealed');
		if (wasRevealed && isAncestor(this.bar.ownerDocument.activeElement, this.bar)) {
			this.left.fire();
		}
	}

	public update(model: DesktopBarModel): void {
		this.state.dataset.phase = model.phase;
		this.sizeButton.replaceChildren(renderIcon(Icons.fitToView), $('span', undefined, model.size), renderIcon(Icons.chevronDown));
		this.sizeButton.setAttribute('aria-label', nls.localize('desktop.pictureSizeIs', 'Picture size: {0}', model.size));
		this.displayButton.hidden = model.display === undefined;
		if (model.display !== undefined) {
			this.displayButton.replaceChildren(renderIcon(Icons.remoteDesktop), $('span', undefined, model.display), renderIcon(Icons.chevronDown));
			this.displayButton.setAttribute('aria-label', nls.localize('desktop.displayIs', 'Display: {0}', model.display));
		}
		this.soundButton.hidden = model.sound === undefined;
		this.soundButton.setAttribute('aria-pressed', String(model.sound === 'muted'));
		this.soundButton.replaceChildren(renderIcon(model.sound === 'muted' ? Icons.audioMuted : Icons.audio));
		describe(this.soundButton, model.sound === 'muted' ? nls.localize('desktop.unmute', 'Unmute Sound') : nls.localize('desktop.mute', 'Mute Sound'));
		this.privacyButton.hidden = model.privacy === undefined;
		this.privacyButton.setAttribute('aria-pressed', String(model.privacy === true));
		this.privacyButton.setAttribute('aria-label', nls.localize('desktop.privacyMode', 'Privacy mode'));
		this.privacyButton.replaceChildren(renderIcon(Icons.privacy, { variant: model.privacy ? 'filled' : 'outlined' }), $('span', undefined, nls.localize('desktop.privacy', 'Privacy')));
		this.privacyButton.title = model.privacy
			? nls.localize('desktop.privacyOn', 'Privacy mode is on: the screen there is blank and its keyboard and mouse are blocked')
			: nls.localize('desktop.privacyOff', 'Privacy mode: blank the screen there and block its keyboard and mouse');
		this.recordingButton.hidden = model.recording === undefined;
		if (model.recording !== undefined) {
			this.recordingButton.replaceChildren($('span.desktop-bar-recording-dot', { 'aria-hidden': 'true' }), $('span', undefined, model.recording));
			describe(this.recordingButton, nls.localize('desktop.stopRecordingAt', 'Stop Recording ({0})', model.recording));
		}
		this.fullScreenButton.replaceChildren(renderIcon(model.fullScreen ? Icons.exitFullScreen : Icons.fullScreen));
		describe(this.fullScreenButton, model.fullScreen ? nls.localize('desktop.exitFullScreen', 'Exit Full Screen') : nls.localize('desktop.fullScreen', 'Full Screen'));
	}
}

/** Names an icon-only button for its tooltip and assistive technology alike. */
function describe(button: HTMLButtonElement, label: string): void {
	button.title = label;
	button.setAttribute('aria-label', label);
}
