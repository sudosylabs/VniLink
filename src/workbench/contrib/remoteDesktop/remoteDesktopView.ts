// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import * as nls from '../../../nls.js';
import { $, addDisposableListener, append, DisposableResizeObserver, EventType, getWindow } from '../../../base/browser/dom.js';
import { Separator, toAction, type IAction } from '../../../base/common/actions.js';
import { Icons } from '../../../base/common/icons.js';
import { isMacintosh, isWindows } from '../../../base/common/platform.js';
import { Disposable, DisposableStore, MutableDisposable, toDisposable } from '../../../base/common/lifecycle.js';
import { IConfigurationService } from '../../../platform/configuration/common/configuration.js';
import { IMenuService } from '../../../platform/actions/common/actions.js';
import { IContextKeyService } from '../../../platform/contextkey/common/contextkey.js';
import type { DesktopAudioStatus } from '../../../platform/engine/common/dataChannel.js';
import type { DisplayInfo, OperatingSystem, RecordingStatus } from '../../../platform/engine/common/engine.js';
import { IInstantiationService } from '../../../platform/instantiation/common/instantiation.js';
import { hidUsage } from '../../../platform/engine/common/keyUsage.js';
import { INativeHostService } from '../../../platform/native/common/native.js';
import { desktopSwapModifiersKey } from '../../../platform/state/common/uiState.js';
import type { DeviceSummary } from '../../services/devices/deviceService.js';
import { INativeRuntimeService } from '../../services/nativeRuntime/nativeRuntimeService.js';
import { formatElapsed, SessionRecording, type RecordingStartFailure } from '../../services/recording/sessionRecording.js';
import { IRemoteDesktopService, type DesktopEnd, type DesktopFailure, type DesktopInput, type DesktopPicture, type DesktopPointer, type DesktopPointerShape, type DesktopRegion, type IDesktopSession } from '../../services/remoteDesktop/remoteDesktopService.js';
import { ISystemActionService } from '../../services/systemActions/systemActionService.js';
import { ITabService, type WorkbenchTab } from '../../services/tabs/tabService.js';
import { IUIStateService } from '../../services/uiState/uiStateService.js';
import { DesktopSessionBar, type DesktopBarDelegate, type DesktopBarModel } from './desktopSessionBar.js';
import { DesktopStatsOverlay, type DesktopPhase } from './desktopStatsOverlay.js';
import { RemoteDesktopMenu } from './remoteDesktopMenu.js';
import { StillCompositor } from './stillCompositor.js';

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
 * `next` in place of `first`, which it replaces before it is drawn: a change
 * when either is, where either says it changed. A repeat changes nothing; a
 * still, or a change that cannot say, may have changed anything.
 */
function replacing(first: DesktopPicture, next: DesktopPicture): DesktopPicture {
	if (first.role !== 'changed' && next.role !== 'changed') {
		return next;
	}
	const where = (picture: DesktopPicture): readonly DesktopRegion[] | undefined => picture.role === 'unchanged' ? [] : picture.role === 'changed' ? picture.regions : undefined;
	const before = where(first);
	const after = where(next);
	return { ...next, role: 'changed', regions: before && after ? [...before, ...after] : undefined };
}

/**
 * Holds the newest decoded picture until the screen's next frame, then
 * draws it: pictures that arrive faster than the screen refreshes replace
 * the one waiting, so the shown picture is never behind the decoder. A
 * still waiting is not replaced by a repeat of the same screen, and a
 * picture that replaces another carries its changes too.
 */
class NextFramePresenter extends Disposable {
	private pending: DesktopPicture | undefined;
	private handle: number | undefined;

	constructor(
		private readonly targetWindow: Window,
		private readonly present: (picture: DesktopPicture) => void,
		private readonly skip: (picture: DesktopPicture) => void,
	) {
		super();
	}

	public show(picture: DesktopPicture): void {
		if (picture.role === 'unchanged' && this.pending?.role === 'still') {
			picture.frame.close();
			return;
		}
		let next = picture;
		if (this.pending) {
			next = replacing(this.pending, picture);
			this.pending.frame.close();
			this.skip(this.pending);
		}
		this.pending = next;
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

/**
 * How the picture is sized in its tab: fitted, never larger than its own
 * pixels; stretched to fill the tab; or zoomed, where 1 puts each pixel of
 * the picture on one pixel of this screen.
 */
export type PictureSize = 'fit' | 'stretch' | number;

/** The zooms offered beside fitting and stretching; 1 is the original size. */
const zooms: readonly number[] = [0.5, 0.75, 1, 1.5, 2];

function pictureSizeFrom(value: unknown): PictureSize {
	if (value === 'fit' || value === 'stretch') {
		return value;
	}
	if (value === 'original') {
		return 1;
	}
	const zoom = Number(value);
	return zooms.includes(zoom) ? zoom : 'fit';
}

interface Size {
	readonly width: number;
	readonly height: number;
}

/**
 * The picture's size on screen in CSS pixels, for a picture of `width` ×
 * `height` pixels in a stage of `stage` CSS pixels on a screen of `ratio`
 * device pixels per CSS pixel. Whole device pixels, so a picture at its
 * original size is drawn without resampling.
 */
export function shownSize(size: PictureSize, width: number, height: number, stage: Size, ratio: number): Size {
	const fill = Math.min(stage.width * ratio / width, stage.height * ratio / height);
	const room = Number.isFinite(fill) && fill > 0 ? fill : 1;
	const scale = size === 'fit' ? Math.min(1, room) : size === 'stretch' ? room : size;
	return { width: Math.max(1, Math.floor(width * scale)) / ratio, height: Math.max(1, Math.floor(height * scale)) / ratio };
}

function pictureSizeLabel(size: PictureSize): string {
	switch (size) {
		case 'fit': return nls.localize('desktop.pictureFit', 'Fit');
		case 'stretch': return nls.localize('desktop.pictureStretch', 'Stretch');
		default:
			return size === 1 ? nls.localize('desktop.pictureOriginal', 'Original size') : nls.localize('desktop.pictureZoom', '{0}%', Math.round(size * 100));
	}
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
		case 'hostDisconnected': return nls.localize('desktop.hostDisconnected', '{0} ended the connection.', name);
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
		case 'hostStopped': case 'hostDisconnected': case 'revoked': case 'cantShow':
			return '';
	}
}

/**
 * Whether Control and Command are exchanged with a computer running `other`
 * by default: when exactly one of the two is a Mac, whose Command does what
 * Control does elsewhere. An unknown system keeps each key as itself.
 */
function automaticSwap(other: OperatingSystem | null): boolean {
	return other !== null && other !== 'other' && isMacintosh !== (other === 'macos');
}

/**
 * Why the other computer's sound isn't heard, from the engine's error
 * category and whether this computer's own output failed.
 */
function soundUnavailableReason(name: string, sound: DesktopAudioStatus): string {
	if (sound.playbackFailed) {
		return nls.localize('desktop.soundPlaybackFailed', 'This computer’s speakers couldn’t play the sound of {0}.', name);
	}
	switch (sound.error?.code) {
		case 'Unauthorized': return nls.localize('desktop.soundNotShared', '{0} doesn’t share its sound.', name);
		case 'Busy': return nls.localize('desktop.soundBusy', '{0} is playing sound from another computer.', name);
		case 'Unsupported': return nls.localize('desktop.soundUnsupported', '{0} has no sound to share. On a Mac, VniLink needs Screen & System Audio Recording.', name);
		case undefined: default: return nls.localize('desktop.soundStopped', 'The sound of {0} stopped.', name);
	}
}

/** Why a reconnect is under way, as the overlay's body says it. */
function reconnectReason(name: string, problem: DesktopProblem | undefined): string {
	switch (problem) {
		case 'hostEncoderFailed': case 'hostCaptureFailed': case 'peerClosed': return `${problemMessage(name, problem)} ${nls.localize('desktop.reconnectingHost', 'Keys and the pointer stay here until its screen is back.')}`;
		case 'relayUnreachable': return nls.localize('desktop.reconnectingRelay', 'The relayed connection dropped. Keys and the pointer stay here until it’s back.');
		case 'disconnected': case 'turnedOff': case 'needsPermission': case 'unsupported': case 'unavailable': case 'failed':
		case 'linkLost': case 'hostPermission': case 'displayGone': case 'hostStopped': case 'hostDisconnected': case 'hostShutdown': case 'revoked': case 'cantShow':
		case undefined:
			return nls.localize('desktop.reconnectingBody', 'The connection dropped. Keys and the pointer stay here until it’s back.');
	}
}

/**
 * A paired computer's screen in its tab, sized as the user chooses (fitted
 * by default), in the tab or full screen; when this computer controls it,
 * the keyboard, pointer and wheel go to it while the screen has focus. One
 * tab per computer. A dropped connection is tried again by itself, over the
 * last picture, dimmed.
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
	/** How the picture is doing, over it. */
	private readonly stats: DesktopStatsOverlay;
	/** The session's controls, off the picture until wanted. */
	private readonly bar: DesktopSessionBar;
	private readonly recording: SessionRecording;
	private pictureSize: PictureSize;
	private fullScreen = false;
	private phase: DesktopPhase = 'connecting';
	/** The other computer's sound here, once its channel says. */
	private sound: DesktopAudioStatus | undefined;
	private readingSound = false;
	/** Privacy mode on the other computer, as it last answered. */
	private privacy = false;
	private settingPrivacy = false;
	/** Whether a still picture, in full colour, was drawn in this session. */
	private stillShown = false;
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
	/** Keeps a still where the screen has not changed since it. */
	private readonly stills = new StillCompositor();
	/** The end of the running picture that the current reconnect answers. */
	private reconnectingFor: DesktopProblem | undefined;
	/** The display shown, and its bounds, as the other computer reports them at the picture's revision. */
	private shown: { readonly display: string; readonly revision: string } | undefined;
	/** The shown display's bounds, once the other computer's topology names it at the picture's revision. */
	private display: DisplayInfo | undefined;
	/** Every display of the other computer at the shown picture's revision, to choose from. */
	private displays: readonly DisplayInfo[] = [];
	/** A display chosen here and not yet shown; pictures of the old one don't select it back meanwhile. */
	private requestedDisplay: string | undefined;
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
	/** Whether this desktop exchanges Control and Command, as last sent. */
	private swapModifiers = false;
	/** The exchange chosen from this desktop's menu for this computer, remembered between runs. */
	private swapChosen: boolean | undefined;
	/** Whether the picture has the keyboard, with this computer's own shortcuts, as last said. */
	private keyboardOwned = false;

	constructor(
		private readonly device: DeviceSummary,
		/** Whether this computer controls the other one, or only views its screen. */
		public readonly control: boolean,
		@IRemoteDesktopService private readonly remoteDesktop: IRemoteDesktopService,
		@ITabService private readonly tabService: ITabService,
		@INativeHostService private readonly nativeHost: INativeHostService,
		@IConfigurationService private readonly configurationService: IConfigurationService,
		@IInstantiationService instantiationService: IInstantiationService,
		@ISystemActionService private readonly systemActions: ISystemActionService,
		@IMenuService private readonly menuService: IMenuService,
		@IContextKeyService private readonly contextKeyService: IContextKeyService,
		@IUIStateService private readonly uiState: IUIStateService,
		@INativeRuntimeService nativeRuntime: INativeRuntimeService,
	) {
		super();
		this.pictureSize = pictureSizeFrom(configurationService.getValue<unknown>('desktop.pictureSize'));
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
		const resize = this._register(new DisposableResizeObserver('remoteDesktopStage', () => {
			this.layout();
		}, getWindow(this.element)));
		this._register(resize.observe(this.stage));
		this._register(addDisposableListener(this.stage, EventType.SCROLL, () => {
			this.showPointer();
		}));
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
		this.stats = this._register(instantiationService.createInstance(DesktopStatsOverlay, device.id, device.name, control));
		this.element.append(this.stats.element);
		this.bar = this._register(instantiationService.createInstance(DesktopSessionBar, this.element, device.name, this.barDelegate()));
		this.element.append(this.bar.element);
		this.recording = this._register(new SessionRecording(() => this.remoteDesktop.startRecording(this.device.id), nativeRuntime));
		void this.uiState.get('desktop.showStats').then(shown => {
			this.stats.shown = shown !== false;
		});
		if (control) {
			void this.uiState.get(desktopSwapModifiersKey(device.id)).then(chosen => {
				if (!this._store.isDisposed) {
					this.swapChosen = chosen;
					this.applySwap();
				}
			});
			this._register(configurationService.onDidChangeConfiguration(event => {
				if (event.affectsConfiguration('input.swapModifiers')) {
					this.applySwap();
				}
			}));
		}
		this.registerViewControls();
		this.updateBar();
		if (control) {
			this.registerInput();
		}
		this._register(this.tabService.onDidChange(() => {
			if (this.tabService.activeTab !== this) {
				this.releaseInput();
				void this.setFullScreen(false);
			}
		}));
		this._register(toDisposable(() => {
			this.ownKeyboard(false);
			this.releaseInput();
		}));
		void this.connect();
	}

	public static idFor(deviceId: string): string {
		return `${RemoteDesktopView.PREFIX}${deviceId}`;
	}

	private registerViewControls(): void {
		this._register(this.bar.onDidLeave(() => {
			this.focusScreen();
		}));
		this._register(this.recording.onDidChange(() => {
			this.updateBar();
		}));
		this._register(this.recording.onDidEnd(status => {
			this.stats.notice(recordingEndMessage(status));
		}));
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
			// Privacy mode is this tab's: closing it gives the other computer back its screen and input.
			if (this.privacy) {
				void this.systemActions.setPrivacy(this.device.id, false);
			}
		}));
	}

	/** What the session bar's controls do. */
	private barDelegate(): DesktopBarDelegate {
		return {
			sizeActions: () => this.sizeActions(),
			displayActions: () => this.displayActions(),
			moreActions: () => this.moreActions(),
			toggleSound: () => void this.toggleSound(),
			togglePrivacy: () => void this.togglePrivacy(),
			stopRecording: () => void this.toggleRecording(),
			toggleFullScreen: () => void this.setFullScreen(!this.fullScreen),
		};
	}

	private sizeActions(): IAction[] {
		const size = (value: PictureSize): IAction => toAction({
			id: `desktop.size.${String(value)}`,
			label: pictureSizeLabel(value),
			checked: this.pictureSize === value,
			run: () => {
				this.pictureSize = value;
				this.layout();
				this.updateBar();
			},
		});
		return [
			size('fit'),
			size('stretch'),
			new Separator(),
			...zooms.map(size),
			new Separator(),
			toAction({
				id: 'desktop.fullScreen',
				label: this.fullScreen ? nls.localize('desktop.exitFullScreen', 'Exit Full Screen') : nls.localize('desktop.fullScreen', 'Full Screen'),
				run: () => void this.setFullScreen(!this.fullScreen),
			}),
		];
	}

	private displayActions(): IAction[] {
		return this.displays.map((display, index) => toAction({
			id: `desktop.display.${display.id}`,
			label: displayLabel(display, index),
			checked: (this.requestedDisplay ?? this.shown?.display) === display.id,
			run: () => {
				this.selectDisplay(display.id);
			},
		}));
	}

	/** Asks the other computer for another of its displays; its pictures follow. */
	private selectDisplay(id: string): void {
		if (!this.control || id === (this.requestedDisplay ?? this.shown?.display)) {
			return;
		}
		this.requestedDisplay = id;
		this.selected = id;
		this.session.value?.send({ type: 'selectDisplays', ids: [id] });
		this.updateBar();
	}

	private moreActions(): IAction[] {
		const own: IAction[] = [
			toAction({
				id: 'desktop.record',
				label: this.recording.recording ? nls.localize('desktop.stopRecording', 'Stop Recording') : nls.localize('desktop.record', 'Record Screen'),
				enabled: this.phase === 'live' && !this.recording.stopping,
				run: () => void this.toggleRecording(),
			}),
		];
		const computer = this.menuService.getMenuActions(RemoteDesktopMenu, this.contextKeyService, { arg: this.device.id, shouldForwardArgs: true })
			.flatMap(([, actions]) => [new Separator(), ...actions]);
		// Offered where the keyboards differ, and wherever the exchange is on, so it can be turned off.
		if (this.control && (automaticSwap(this.device.os) || this.swapModifiers)) {
			own.push(toAction({
				id: 'desktop.swapModifiers',
				label: nls.localize('desktop.swapModifiers', 'Swap Control and Command'),
				checked: this.swapModifiers,
				run: () => {
					this.swapChosen = !this.swapModifiers;
					void this.uiState.set(desktopSwapModifiersKey(this.device.id), this.swapChosen);
					this.applySwap();
				},
			}));
		}
		return [
			...own,
			...computer,
			new Separator(),
			toAction({
				id: 'desktop.showStats',
				label: nls.localize('desktop.showStats', 'Show Picture Details'),
				checked: this.stats.shown,
				run: () => {
					this.stats.shown = !this.stats.shown;
					void this.uiState.set('desktop.showStats', this.stats.shown);
				},
			}),
			toAction({
				id: 'desktop.disconnect',
				label: nls.localize('desktop.disconnect', 'Disconnect'),
				run: () => void this.tabService.close(this.id),
			}),
		];
	}

	private async toggleRecording(): Promise<void> {
		const failure = await this.recording.toggle();
		if (failure && !this._store.isDisposed) {
			this.stats.notice(recordingStartMessage(this.device.name, failure));
		}
	}

	private async toggleSound(): Promise<void> {
		const sound = this.sound;
		if (!sound) {
			return;
		}
		try {
			// Enabling sound that was unavailable asks the other computer again.
			const status = await this.session.value?.audio(sound.phase === 'unavailable' ? { type: 'enabled', enabled: true } : { type: 'muted', muted: !sound.muted });
			this.showSound(status);
		} catch {
			this.stats.notice(nls.localize('desktop.soundFailed', 'The sound couldn’t be changed.'));
		}
	}

	/** Reads the other computer's sound with the stats, one read at a time. */
	private async readSound(): Promise<void> {
		const session = this.session.value;
		if (!session || this.readingSound) {
			return;
		}
		this.readingSound = true;
		try {
			const status = await session.audio({ type: 'status' });
			if (this.session.value === session) {
				this.showSound(status);
			}
		} catch {
			// The next stats read asks again.
		} finally {
			this.readingSound = false;
		}
	}

	private showSound(status: DesktopAudioStatus | undefined): void {
		this.sound = status?.phase === 'stopped' ? undefined : status;
		this.updateBar();
	}

	private async togglePrivacy(): Promise<void> {
		if (!this.control || this.settingPrivacy) {
			return;
		}
		this.settingPrivacy = true;
		try {
			const result = await this.systemActions.setPrivacy(this.device.id, !this.privacy);
			if (this._store.isDisposed) {
				return;
			}
			if ('error' in result) {
				this.stats.notice(nls.localize('desktop.privacyFailed', 'Privacy mode: {0}', result.error));
			} else {
				this.privacy = result.privacy.displayPrivate || result.privacy.inputBlocked;
				this.stats.notice(this.privacy
					? nls.localize('desktop.privacyStarted', 'Privacy mode on: the screen of {0} is blank there and its keyboard and mouse are blocked.', this.device.name)
					: nls.localize('desktop.privacyEnded', 'Privacy mode off.'));
			}
		} finally {
			this.settingPrivacy = false;
			this.updateBar();
		}
	}

	/** Keys go back to the other computer once the controls are put away. */
	private focusScreen(): void {
		if (this.control && this.tabService.activeTab === this && this.phase === 'live') {
			this.canvas.focus();
		}
	}

	/** The sound as the bar shows it; still starting shows nothing yet. */
	private soundModel(): DesktopBarModel['sound'] {
		const sound = this.sound;
		if (!sound || sound.phase === 'stopped' || sound.phase === 'starting') {
			return undefined;
		}
		if (sound.phase === 'unavailable') {
			return { unavailable: soundUnavailableReason(this.device.name, sound) };
		}
		return sound.muted ? 'muted' : 'playing';
	}

	private updateBar(): void {
		const shownIndex = this.displays.findIndex(display => display.id === (this.requestedDisplay ?? this.shown?.display));
		this.bar.update({
			phase: this.phase,
			size: pictureSizeLabel(this.pictureSize),
			display: this.control && this.displays.length > 1 && shownIndex >= 0 ? displayLabel(this.displays[shownIndex], shownIndex, false) : undefined,
			sound: this.soundModel(),
			privacy: this.control && this.phase === 'live' ? this.privacy : undefined,
			recording: this.recording.recording ? formatElapsed(this.recording.elapsedMs) : undefined,
			fullScreen: this.fullScreen,
		});
	}

	/**
	 * The window goes full screen and the screen covers it, the status bar
	 * kept below it: keys go to the other computer, so leaving is a button
	 * there, or the window's own controls.
	 */
	public async setFullScreen(fullScreen: boolean): Promise<void> {
		if (fullScreen === this.fullScreen) {
			return;
		}
		this.applyFullScreen(fullScreen);
		await this.nativeHost.setFullScreen(fullScreen);
	}

	private applyFullScreen(fullScreen: boolean): void {
		this.fullScreen = fullScreen;
		this.element.classList.toggle('desktop-view-fullscreen', fullScreen);
		this.updateBar();
	}

	/** Sizes the picture in its stage as chosen, centred where it is smaller, scrolling where it is larger. */
	private layout(): void {
		const ratio = getWindow(this.element).devicePixelRatio || 1;
		// Fit and Stretch never scroll, so scroll bars left from a larger size, which take room where
		// they are not overlaid, are about to go: their room is the whole stage.
		const whole = this.pictureSize === 'fit' || this.pictureSize === 'stretch';
		const stage = whole
			? { width: this.stage.offsetWidth, height: this.stage.offsetHeight }
			: { width: this.stage.clientWidth, height: this.stage.clientHeight };
		const shown = shownSize(this.pictureSize, this.canvas.width, this.canvas.height, stage, ratio);
		// Whole device pixels for the margins too, so the picture is not split across pixels.
		const margin = (room: number, size: number): string => `${String(Math.max(0, Math.floor((room - size) / 2 * ratio)) / ratio)}px`;
		this.canvas.style.width = `${String(shown.width)}px`;
		this.canvas.style.height = `${String(shown.height)}px`;
		this.canvas.style.marginLeft = margin(stage.width, shown.width);
		this.canvas.style.marginTop = margin(stage.height, shown.height);
		this.showPointer();
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
			this.ownKeyboard(false);
			this.releaseInput();
			this.showProblem(end, true);
		}));
		this.session.value = session;
		this.applySwap(true);
		// A new session takes the keyboard where the picture already has focus; it starts without it.
		const document = this.canvas.ownerDocument;
		const focused = document.activeElement === this.canvas && document.hasFocus();
		this.ownKeyboard(focused, focused);
		this.stills.reset();
		this.remotePointer = undefined;
		this.showPointer();
		this.selected = undefined;
		this.display = undefined;
		this.displays = [];
		this.requestedDisplay = undefined;
		this.shown = undefined;
		this.sound = undefined;
		this.stillShown = false;
		this.lastRead = -Infinity;
		this.retries = 0;
		this.updateBar();
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
		const { frame, display, revision, role, regions } = picture;
		try {
			if (!this.hasPicture || this.overlay.dataset.state) {
				this.hasPicture = true;
				this.hideState();
			}
			if (this.shown?.display !== display || this.shown.revision !== revision) {
				this.shown = { display, revision };
				this.display = undefined;
			}
			// A display chosen here is shown once its pictures arrive; until then the old one's don't undo the choice.
			if (this.requestedDisplay === display) {
				this.requestedDisplay = undefined;
			}
			if (this.control && this.selected !== display && this.requestedDisplay === undefined) {
				this.selected = display;
				this.session.value?.send({ type: 'selectDisplays', ids: [display] });
			}
			if (role === 'still') {
				this.stillShown = true;
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
				this.layout();
			}
			const plan = this.stills.plan(frame, role, this.canvas, regions);
			if (plan === 'whole') {
				this.context.drawImage(frame, 0, 0);
			} else {
				for (const { x, y, width, height } of plan) {
					this.context.drawImage(frame, x, y, width, height, x, y, width, height);
				}
			}
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
			const displays = (await session.displays()).filter(display => display.revision === shown.revision);
			const found = displays.find(display => display.id === shown.display);
			if (found && this.shown === shown) {
				this.display = found;
				this.displays = displays;
			}
		} catch {
			// The next picture asks again.
		} finally {
			this.readingDisplays = false;
		}
		this.updateBar();
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
		this._register(addDisposableListener(this.canvas, EventType.FOCUS, () => {
			this.ownKeyboard(true);
		}));
		this._register(addDisposableListener(this.canvas, EventType.BLUR, () => {
			this.ownKeyboard(false);
			this.releaseInput();
		}));
	}

	/**
	 * Gives the picture the keyboard, or takes it back: while it has it, this
	 * computer's own shortcuts, such as Alt-Tab or Command-Tab, and the
	 * application's, such as Command-C, go to the other computer. Keys taken
	 * that way never reach this window, so a later release lets go of them.
	 */
	private ownKeyboard(owned: boolean, again = false): void {
		const session = this.session.value;
		const wanted = owned && this.control && session !== undefined;
		if (wanted === this.keyboardOwned && !again) {
			return;
		}
		this.keyboardOwned = wanted;
		void this.nativeHost.setRemoteKeyboard(wanted);
		if (session && this.control) {
			if (wanted) {
				this.pressed = true;
			}
			session.send({ type: 'captureSystemKeys', enabled: wanted });
		}
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
	 * Where the picture is on screen: its top-left corner in the viewport,
	 * and CSS pixels per picture pixel. The canvas is the picture's size.
	 */
	private fitted(): { readonly left: number; readonly top: number; readonly scale: number } | undefined {
		if (!this.canvas.width || !this.canvas.height) {
			return undefined;
		}
		const box = this.canvas.getBoundingClientRect();
		const scale = box.width / this.canvas.width;
		if (!(scale > 0)) {
			return undefined;
		}
		return { left: box.left, top: box.top, scale };
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
			// The mark is placed in the stage's content, which scrolls.
			const stage = this.stage.getBoundingClientRect();
			const x = fit.left - stage.left + this.stage.scrollLeft + remote.x * fit.scale - look.hotspotX;
			const y = fit.top - stage.top + this.stage.scrollTop + remote.y * fit.scale - look.hotspotY;
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

	/** Where the pointer is on the other computer's display, through the picture as it is shown. */
	private pointer(event: PointerEvent): void {
		const display = this.display;
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

	/**
	 * Sends whether Control and Command are exchanged: as chosen here for
	 * this computer, else as the setting says. A key held refuses the change,
	 * so held input is released first. `always` sends it to a new session.
	 */
	private applySwap(always = false): void {
		const session = this.session.value;
		if (!this.control || !session) {
			return;
		}
		const setting = this.configurationService.getValue<unknown>('input.swapModifiers');
		const swap = this.swapChosen ?? (setting === 'always' || (setting !== 'never' && automaticSwap(this.device.os)));
		if (swap === this.swapModifiers && !always) {
			return;
		}
		this.swapModifiers = swap;
		this.pressed = true;
		this.releaseInput();
		session.send({ type: 'swapSessionModifiers', enabled: swap });
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
		this.setPhase(state === 'ended' ? 'ended' : state === 'connecting' ? 'connecting' : 'reconnecting');
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
	}

	private setPhase(phase: DesktopPhase): void {
		this.phase = phase;
		this.stats.setPhase(phase);
		if (phase !== 'live') {
			this.sound = undefined;
		}
		this.updateBar();
	}

	/** Reads how the picture is doing, and its sound, once a second while it shows. */
	private watchStats(): void {
		const timer = setInterval(() => {
			void this.session.value?.stats().then(stats => {
				// The picture may have ended while the engine answered.
				if (this.overlay.dataset.state) {
					return;
				}
				this.stats.update(stats, { display: this.display, stillShown: this.stillShown });
			});
			void this.readSound();
		}, 1000);
		this.statsTimer.value = toDisposable(() => {
			clearInterval(timer);
		});
	}

	private hideState(): void {
		this.watchStats();
		delete this.overlay.dataset.state;
		this.overlay.hidden = true;
		this.overlayListeners.clear();
		this.stage.classList.remove('desktop-dimmed');
		this.setPhase('live');
		void this.readPrivacy();
		if (this.control && !this.element.contains(document.activeElement) && this.tabService.activeTab === this) {
			this.canvas.focus();
		}
	}

	/** Whether privacy mode is already on, as after a reconnect; only a controlling tab asks. */
	private async readPrivacy(): Promise<void> {
		if (!this.control) {
			return;
		}
		const result = await this.systemActions.privacy(this.device.id);
		if (!this._store.isDisposed && 'privacy' in result) {
			this.privacy = result.privacy.displayPrivate || result.privacy.inputBlocked;
			this.updateBar();
		}
	}
}

/** A display as its menu names it: its place among the other computer's displays, and its size. */
function displayLabel(display: DisplayInfo, index: number, withSize = true): string {
	return withSize
		? nls.localize('desktop.displayWithSize', 'Display {0} · {1} × {2}', index + 1, display.width, display.height)
		: nls.localize('desktop.displayNumber', 'Display {0}', index + 1);
}

function recordingStartMessage(name: string, failure: RecordingStartFailure): string {
	switch (failure) {
		case 'notAllowed': return nls.localize('desktop.recordingNotAllowed', '{0} doesn’t allow recording its screen.', name);
		case 'disconnected': return nls.localize('desktop.recordingDisconnected', '{0} isn’t connected.', name);
		case 'failed': return nls.localize('desktop.recordingFailed', 'The recording didn’t start.');
	}
}

function recordingEndMessage(status: RecordingStatus | undefined): string {
	switch (status?.failure) {
		case undefined: return nls.localize('desktop.recordingLost', 'The recording stopped.');
		case null: return nls.localize('desktop.recordingSaved', 'Recording saved to the recordings folder.');
		case 'Startup': return nls.localize('desktop.recordingStartup', 'The recording didn’t start.');
		case 'Storage': return nls.localize('desktop.recordingStorage', 'The recordings folder can’t be written to.');
		case 'SlowStorage': return nls.localize('desktop.recordingSlow', 'The recordings folder couldn’t keep up, so the recording stopped.');
		case 'Limit': return nls.localize('desktop.recordingLimit', 'The recording reached its size limit.');
		case 'Media':
		case 'FormatChanged': return nls.localize('desktop.recordingMedia', 'The screen changed size, so the recording stopped.');
	}
}
