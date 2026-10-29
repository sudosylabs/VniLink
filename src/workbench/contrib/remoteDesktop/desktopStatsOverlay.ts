import * as nls from '../../../nls.js';
import { $, addDisposableListener, append, EventType } from '../../../base/browser/dom.js';
import { status as announce } from '../../../base/browser/ui/aria/aria.js';
import { renderIcon } from '../../../base/browser/ui/icon/icon.js';
import { Icons } from '../../../base/common/icons.js';
import { Disposable, MutableDisposable, toDisposable } from '../../../base/common/lifecycle.js';
import type { DesktopCondition } from '../../../platform/engine/common/dataChannel.js';
import type { DisplayInfo } from '../../../platform/engine/common/engine.js';
import { desktopStatsCornerKey, type DesktopStatsCorner } from '../../../platform/state/common/uiState.js';
import { latencyStages, type LatencyStage } from '../../services/remoteDesktop/desktopLatency.js';
import type { DesktopStats } from '../../services/remoteDesktop/remoteDesktopService.js';
import { IUIStateService } from '../../services/uiState/uiStateService.js';

/** How long a notice stays. */
const noticeTime = 4000;

const codecNames: Readonly<Record<string, string>> = { h264: 'H.264', h265: 'HEVC', vp8: 'VP8', vp9: 'VP9', av1: 'AV1' };

/** Where a session stands, as the overlay's dot and first word say it. */
export type DesktopPhase = 'connecting' | 'reconnecting' | 'ended' | 'live';

function bitrateText(kbps: number): string {
	return kbps >= 1000
		? nls.localize('desktop.megabits', '{0} Mb/s', (kbps / 1000).toFixed(1))
		: nls.localize('desktop.kilobits', '{0} kb/s', kbps);
}

/** Why the quality is lowered, or the bitrate held, when it is; empty when nothing holds it. */
export function qualityText(stats: DesktopStats, name: string): string {
	const { status } = stats;
	switch (status.reason) {
		case 'networkPressure': return nls.localize('desktop.lowNetwork', 'Lowered for the network');
		case 'receiverPressure': return nls.localize('desktop.lowHere', 'Lowered: this computer is behind');
		case 'encoderPressure': return nls.localize('desktop.lowThere', 'Lowered: {0} is behind', name);
		case 'recovering': return nls.localize('desktop.recovering', 'Improving');
		case 'warmingUp': case 'stable': case 'idle': break;
	}
	// A cut already says why; a bitrate that is merely held says what holds it.
	switch (status.limit) {
		case 'setting': return nls.localize('desktop.limitSetting', 'At your bandwidth limit');
		case 'relay': return nls.localize('desktop.limitRelay', 'Limited by the relay');
		case 'network': return nls.localize('desktop.limitNetwork', 'Limited by the network');
		case 'encoder': return nls.localize('desktop.limitEncoder', 'Limited by {0}', name);
		case 'receiver': return nls.localize('desktop.limitReceiver', 'Limited by this computer');
		case 'none': return '';
	}
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

/** Conditions that explain a slower picture without being a fault. */
const explanations: ReadonlySet<DesktopCondition> = new Set(['encoder.software', 'decoder.software', 'decoder.recovering']);

function latencyLabel(stage: LatencyStage, name: string): string {
	switch (stage) {
		case 'host': return nls.localize('desktop.latency.host', 'On {0}', name);
		case 'queue': return nls.localize('desktop.latency.queue', 'Waiting here');
		case 'bridge': return nls.localize('desktop.latency.bridge', 'To the window');
		case 'decode': return nls.localize('desktop.latency.decode', 'Decoding');
		case 'draw': return nls.localize('desktop.latency.draw', 'Drawing');
	}
}

/** What the picture's details say beyond its stats: the display shown, and whether a still was. */
export interface DesktopPictureFacts {
	readonly display: DisplayInfo | undefined;
	/** Whether a still picture, in full colour, was shown in this session. */
	readonly stillShown: boolean;
}

/**
 * How the picture is doing, over it: a line of frame rate, round trip and
 * path, what slows it when something does, and on request every detail.
 * Barely there until the pointer or focus reaches it; dragged to any
 * corner, which is remembered for that computer.
 */
export class DesktopStatsOverlay extends Disposable {
	public readonly element: HTMLElement;
	private readonly grip: HTMLButtonElement;
	private readonly role: HTMLElement;
	private readonly summary: HTMLElement;
	private readonly toggle: HTMLButtonElement;
	private readonly condition: HTMLElement;
	private readonly details: HTMLElement;
	private readonly message: HTMLElement;
	private readonly messageTimer = this._register(new MutableDisposable());
	private corner: DesktopStatsCorner = 'topRight';
	private expanded = false;
	private phase: DesktopPhase = 'connecting';
	private stats: DesktopStats | undefined;
	private facts: DesktopPictureFacts = { display: undefined, stillShown: false };
	/** The condition last shown and announced. */
	private shownCondition: DesktopCondition | undefined;
	private drag: { readonly pointerId: number; readonly dx: number; readonly dy: number; moved: boolean } | undefined;

	constructor(
		private readonly deviceId: string,
		private readonly name: string,
		private readonly control: boolean,
		@IUIStateService private readonly uiState: IUIStateService,
	) {
		super();
		this.element = $('section.desktop-stats', { 'aria-label': nls.localize('desktop.statsLabel', 'Picture details') });
		const line = append(this.element, $('.desktop-stats-line'));
		this.grip = append(line, $<HTMLButtonElement>('button.desktop-stats-grip', { type: 'button' }));
		this.grip.title = nls.localize('desktop.statsMove', 'Move the picture details: drag, or use the arrow keys');
		this.grip.setAttribute('aria-label', nls.localize('desktop.statsMoveLabel', 'Move picture details'));
		append(this.grip, renderIcon(Icons.more));
		this.role = append(line, $('span.desktop-stats-role'));
		this.summary = append(line, $('span.desktop-stats-summary'));
		this.toggle = append(line, $<HTMLButtonElement>('button.desktop-stats-toggle', { type: 'button', 'aria-expanded': 'false' }));
		this.condition = append(this.element, $('p.desktop-stats-condition'));
		this.details = append(this.element, $('.desktop-stats-details'));
		this.message = append(this.element, $('p.desktop-stats-notice', { role: 'status' }));
		this.details.id = `desktop-stats-details-${deviceId}`;
		this.toggle.setAttribute('aria-controls', this.details.id);
		this.registerListeners();
		this.render();
		void this.uiState.get(desktopStatsCornerKey(deviceId)).then(corner => {
			if (corner && !this.drag && !this._store.isDisposed) {
				this.place(corner);
			}
		});
		this.place(this.corner);
	}

	private registerListeners(): void {
		this._register(addDisposableListener(this.toggle, EventType.CLICK, () => {
			this.expanded = !this.expanded;
			this.render();
		}));
		this._register(addDisposableListener(this.grip, EventType.KEY_DOWN, event => {
			const corner = cornerAfter(this.corner, event.key);
			if (corner) {
				event.preventDefault();
				this.moveTo(corner);
			}
		}));
		this._register(addDisposableListener(this.element, EventType.POINTER_DOWN, event => {
			// The details' toggle is a button of its own; anywhere else picks the overlay up.
			if (event.button !== 0 || (event.target instanceof Element && event.target.closest('.desktop-stats-toggle'))) {
				return;
			}
			const bounds = this.element.getBoundingClientRect();
			this.drag = { pointerId: event.pointerId, dx: event.clientX - bounds.left, dy: event.clientY - bounds.top, moved: false };
			this.element.setPointerCapture(event.pointerId);
		}));
		this._register(addDisposableListener(this.element, EventType.POINTER_MOVE, event => {
			const drag = this.drag;
			const parent = this.element.offsetParent;
			if (drag?.pointerId !== event.pointerId || !(parent instanceof HTMLElement)) {
				return;
			}
			const room = parent.getBoundingClientRect();
			const left = clamp(event.clientX - drag.dx - room.left, 0, room.width - this.element.offsetWidth);
			const top = clamp(event.clientY - drag.dy - room.top, 0, room.height - this.element.offsetHeight);
			drag.moved = true;
			this.element.classList.add('dragging');
			this.element.style.left = `${String(left)}px`;
			this.element.style.top = `${String(top)}px`;
			this.element.style.right = this.element.style.bottom = 'auto';
		}));
		const drop = (event: PointerEvent): void => {
			const drag = this.drag;
			if (drag?.pointerId !== event.pointerId) {
				return;
			}
			this.drag = undefined;
			this.element.classList.remove('dragging');
			const parent = this.element.offsetParent;
			if (!drag.moved || !(parent instanceof HTMLElement)) {
				this.place(this.corner);
				return;
			}
			// It settles in the corner nearest its centre.
			const room = parent.getBoundingClientRect();
			const bounds = this.element.getBoundingClientRect();
			const right = bounds.left + bounds.width / 2 > room.left + room.width / 2;
			const bottom = bounds.top + bounds.height / 2 > room.top + room.height / 2;
			this.moveTo(bottom ? (right ? 'bottomRight' : 'bottomLeft') : (right ? 'topRight' : 'topLeft'));
		};
		this._register(addDisposableListener(this.element, EventType.POINTER_UP, drop));
		this._register(addDisposableListener(this.element, 'pointercancel', drop));
	}

	private moveTo(corner: DesktopStatsCorner): void {
		this.place(corner);
		void this.uiState.set(desktopStatsCornerKey(this.deviceId), corner);
	}

	private place(corner: DesktopStatsCorner): void {
		this.corner = corner;
		this.element.dataset.corner = corner;
		this.element.style.left = this.element.style.top = this.element.style.right = this.element.style.bottom = '';
	}

	public setPhase(phase: DesktopPhase): void {
		this.phase = phase;
		if (phase !== 'live') {
			this.stats = undefined;
		}
		this.render();
	}

	/** Whether the details show; hidden, the overlay still says what just happened. */
	public get shown(): boolean {
		return !this.element.classList.contains('details-off');
	}

	public set shown(shown: boolean) {
		this.element.classList.toggle('details-off', !shown);
	}

	/** Says what just happened, such as a recording saved, for a few seconds; shown even while the details are hidden. */
	public notice(text: string): void {
		this.message.textContent = text;
		this.element.classList.add('noticing');
		const timer = setTimeout(() => {
			this.message.textContent = '';
			this.element.classList.remove('noticing');
		}, noticeTime);
		this.messageTimer.value = toDisposable(() => {
			clearTimeout(timer);
		});
	}

	public update(stats: DesktopStats | undefined, facts: DesktopPictureFacts): void {
		this.stats = stats;
		this.facts = facts;
		this.render();
	}

	private render(): void {
		const stats = this.phase === 'live' ? this.stats : undefined;
		this.element.dataset.phase = this.phase;
		this.role.textContent = this.roleText();
		const conditions = stats?.status.conditions ?? [];
		const relayed = conditions.includes('relay.path');
		this.summary.textContent = stats
			? [
				nls.localize('desktop.fps', '{0} fps', stats.status.receivedFps),
				nls.localize('desktop.rtt', '{0} ms', stats.status.rttMs),
				relayed ? nls.localize('desktop.pathRelay', 'Relay') : nls.localize('desktop.pathDirect', 'Direct'),
			].join(' · ')
			: '';
		this.summary.hidden = !stats;
		this.toggle.hidden = !stats;
		const toggleLabel = this.expanded ? nls.localize('desktop.statsHide', 'Hide details') : nls.localize('desktop.statsShow', 'Show details');
		this.toggle.title = toggleLabel;
		this.toggle.setAttribute('aria-label', toggleLabel);
		this.toggle.setAttribute('aria-expanded', String(this.expanded && !!stats));
		this.toggle.replaceChildren(renderIcon(this.expanded ? Icons.chevronUp : Icons.chevronDown));
		this.renderCondition(conditions);
		this.details.hidden = !this.expanded || !stats;
		this.element.classList.toggle('expanded', this.expanded && !!stats);
		if (stats && this.expanded) {
			this.renderDetails(stats);
		}
	}

	private roleText(): string {
		switch (this.phase) {
			case 'connecting': return nls.localize('desktop.statusConnecting', 'Connecting');
			case 'reconnecting': return nls.localize('desktop.statusReconnecting', 'Reconnecting');
			case 'ended': return nls.localize('desktop.notConnected', 'Not connected');
			case 'live': return this.control ? nls.localize('desktop.statusControlling', 'Controlling') : nls.localize('desktop.statusViewing', 'Viewing');
		}
	}

	/**
	 * The worst condition that needs attention, in one line, and said aloud
	 * once when it changes; every held one in its hover. A relay alone is
	 * not shown: the path already says it.
	 */
	private renderCondition(conditions: readonly DesktopCondition[]): void {
		const worst = conditions.find(condition => !informational.has(condition));
		this.condition.hidden = worst === undefined;
		this.condition.textContent = worst ? conditionText(worst, this.name) : '';
		this.condition.title = conditions.map(condition => conditionText(condition, this.name)).join('\n');
		this.condition.classList.toggle('explanation', worst !== undefined && explanations.has(worst));
		if (worst) {
			this.condition.dataset.condition = worst;
		} else {
			delete this.condition.dataset.condition;
		}
		if (worst !== this.shownCondition) {
			this.shownCondition = worst;
			if (worst) {
				announce(conditionText(worst, this.name));
			}
		}
	}

	private renderDetails(stats: DesktopStats): void {
		const { status } = stats;
		const display = this.facts.display;
		const codec = codecNames[stats.codec] ?? stats.codec.toUpperCase();
		const quality = qualityText(stats, this.name);
		const relayed = status.conditions.includes('relay.path');
		const rows: [string, string][] = [
			[nls.localize('desktop.detailPicture', 'Picture'), display
				? nls.localize('desktop.detailPictureSize', '{0} × {1} · {2} fps', display.width, display.height, status.receivedFps)
				: nls.localize('desktop.fps', '{0} fps', status.receivedFps)],
			// The stream is always 4:2:0; only still pictures carry full colour.
			[nls.localize('desktop.detailCodec', 'Codec'), this.facts.stillShown
				? nls.localize('desktop.detailCodecStills', '{0} · 4:2:0, stills 4:4:4', codec)
				: nls.localize('desktop.detailCodecStream', '{0} · 4:2:0', codec)],
			[nls.localize('desktop.detailEncoder', 'Encoder'), status.conditions.includes('encoder.software')
				? nls.localize('desktop.encoderSoftware', 'Software, on {0}', this.name)
				: nls.localize('desktop.encoderHardware', 'Hardware, on {0}', this.name)],
			[nls.localize('desktop.detailDecoder', 'Decoder'), stats.softwareDecoder
				? nls.localize('desktop.decoderSoftware', 'Software, in this window')
				: nls.localize('desktop.decoderHardware', 'Hardware, in this window')],
			[nls.localize('desktop.detailBitrate', 'Bitrate'), quality ? `${bitrateText(status.receivedKbps)} · ${quality}` : bitrateText(status.receivedKbps)],
			[nls.localize('desktop.detailConnection', 'Connection'), relayed
				? nls.localize('desktop.connectionRelay', 'Relay · {0} ms round trip', status.rttMs)
				: nls.localize('desktop.connectionDirect', 'Direct · {0} ms round trip', status.rttMs)],
		];
		const list = $('dl.desktop-stats-facts');
		for (const [term, value] of rows) {
			append(list, $('dt', undefined, term));
			append(list, $('dd', undefined, value));
		}
		const timing = $('dl.desktop-stats-latency');
		append(timing, $('dt.desktop-stats-heading', undefined, nls.localize('desktop.latencyHeading', 'Latency, typical · 95%')));
		append(timing, $('dd.desktop-stats-heading'));
		append(timing, $('dt', undefined, nls.localize('desktop.latency.networkLabel', 'Network')));
		append(timing, $('dd', undefined, nls.localize('desktop.latency.eachWay', '{0} ms each way', Math.round(status.rttMs / 2))));
		for (const stage of latencyStages) {
			const latency = stats.latency[stage];
			if (latency) {
				append(timing, $('dt', undefined, latencyLabel(stage, this.name)));
				append(timing, $('dd', undefined, nls.localize('desktop.latency.times', '{0} · {1} ms', latency.p50.toFixed(1), latency.p95.toFixed(1))));
			}
		}
		const parts: HTMLElement[] = [list, timing];
		if (stats.skipped > 0) {
			parts.push($('p.desktop-stats-note', undefined, nls.localize('desktop.latency.skipped', '{0} pictures replaced by newer ones before drawing', stats.skipped)));
		}
		this.details.replaceChildren(...parts);
	}
}

function clamp(value: number, min: number, max: number): number {
	return Math.max(min, Math.min(Math.max(min, max), value));
}

/** The corner an arrow key moves to from `corner`, or `undefined` for other keys. */
function cornerAfter(corner: DesktopStatsCorner, key: string): DesktopStatsCorner | undefined {
	const top = corner === 'topLeft' || corner === 'topRight';
	const left = corner === 'topLeft' || corner === 'bottomLeft';
	const at = (isTop: boolean, isLeft: boolean): DesktopStatsCorner => isTop ? (isLeft ? 'topLeft' : 'topRight') : (isLeft ? 'bottomLeft' : 'bottomRight');
	switch (key) {
		case 'ArrowUp': return at(true, left);
		case 'ArrowDown': return at(false, left);
		case 'ArrowLeft': return at(top, true);
		case 'ArrowRight': return at(top, false);
		default: return undefined;
	}
}
