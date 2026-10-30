// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import './media/devicePairingPopover.css';
import * as nls from '../../../nls.js';
import { addDisposableListener, EventType, getWindow, h, isAncestor, triggerDownload, triggerUpload } from '../../../base/browser/dom.js';
import { StandardKeyboardEvent } from '../../../base/browser/keyboardEvent.js';
import { Button } from '../../../base/browser/ui/button/button.js';
import { CodeInput } from '../../../base/browser/ui/codeInput/codeInput.js';
import { ContextView, ContextViewDOMPosition } from '../../../base/browser/ui/contextview/contextview.js';
import { IntervalTimer } from '../../../base/common/async.js';
import { KeyCode } from '../../../base/common/keyCodes.js';
import { AnchorAlignment } from '../../../base/common/layout.js';
import { Disposable, DisposableStore, toDisposable } from '../../../base/common/lifecycle.js';
import { createDecorator } from '../../../platform/instantiation/common/instantiation.js';
import { defaultButtonStyles, defaultCodeInputStyles } from '../../../platform/theme/browser/defaultStyles.js';
import { IDevicePairingService, type PairingState } from '../../services/devices/devicePairingService.js';
import { IDeviceService } from '../../services/devices/deviceService.js';

/** Which pairing action opened the popover: inviting from here, or opening another computer's invitation. */
export type PairingPopoverKind = 'create' | 'import';

export const IDevicePairingPopover = createDecorator<IDevicePairingPopover>('devicePairingPopover');

/** The pairing popover under the Devices page's pairing buttons. */
export interface IDevicePairingPopover {
	readonly _serviceBrand: undefined;
	/** Shows pairing under `anchor`, or closes it when that action's popover is already open. */
	toggle(anchor: HTMLElement, kind: PairingPopoverKind): void;
	hide(): void;
}

/** Whether a hide returns focus to the button that opened it, as Escape and Done do. */
interface HideData {
	readonly restoreFocus: boolean;
}

function isHideData(value: unknown): value is HideData {
	return typeof value === 'object' && value !== null && 'restoreFocus' in value && value.restoreFocus === true;
}

/**
 * Shows `IDevicePairingService` in a context view anchored to the Devices
 * page's button. Closing it leaves a live invitation waiting for the other computer.
 */
export class DevicePairingPopover extends Disposable implements IDevicePairingPopover {
	declare public readonly _serviceBrand: undefined;
	private contextView: ContextView | undefined;
	private open: { readonly anchor: HTMLElement; readonly kind: PairingPopoverKind } | undefined;
	private readonly openListeners = this._register(new DisposableStore());

	constructor(
		@IDevicePairingService private readonly pairing: IDevicePairingService,
		@IDeviceService private readonly devices: IDeviceService,
	) {
		super();
	}

	public toggle(anchor: HTMLElement, kind: PairingPopoverKind): void {
		if (this.open?.anchor === anchor && this.open.kind === kind) {
			this.hide();
			return;
		}
		// The other action's popover closes first, so its teardown cannot touch this one.
		this.hide();
		const { step, busy } = this.pairing.state;
		if (kind === 'create' && !busy && step.kind !== 'invitation' && step.kind !== 'approval') {
			void this.pairing.createInvitation();
		} else if (kind === 'import' && (step.kind === 'paired' || step.kind === 'ended')) {
			this.pairing.reset();
		}
		const window = getWindow(anchor);
		const container = window.document.body;
		this.contextView ??= this._register(new ContextView(container, ContextViewDOMPosition.ABSOLUTE));
		this.contextView.setContainer(container, ContextViewDOMPosition.ABSOLUTE);
		this.open = { anchor, kind };
		// A press, not a click: saving and choosing files click hidden elements in the body.
		this.openListeners.add(addDisposableListener(window, EventType.POINTER_DOWN, event => {
			const target = event.target instanceof Node ? event.target : null;
			if (!isAncestor(target, this.contextView?.getViewElement() ?? null) && !isAncestor(target, anchor)) {
				this.hide();
			}
		}, true));
		let content: PairingPopoverContent | undefined;
		this.contextView.show({
			getAnchor: () => anchor,
			anchorAlignment: AnchorAlignment.RIGHT,
			render: view => content = new PairingPopoverContent(view, kind, this.pairing, this.devices, () => {
				this.hide({ restoreFocus: true });
			}),
			focus: () => {
				content?.focus();
			},
			onHide: data => {
				this.onHide(anchor, data);
			},
		});
		// Captured: buttons inside stop Escape from bubbling.
		this.openListeners.add(addDisposableListener(this.contextView.getViewElement(), EventType.KEY_DOWN, event => {
			if (new StandardKeyboardEvent(event).equals(KeyCode.Escape)) {
				event.preventDefault();
				event.stopPropagation();
				this.hide({ restoreFocus: true });
			}
		}, true));
	}

	public hide(data: HideData = { restoreFocus: false }): void {
		this.contextView?.hide(data);
	}

	private onHide(anchor: HTMLElement, data: unknown): void {
		this.open = undefined;
		this.openListeners.clear();
		const kind = this.pairing.state.step.kind;
		if (kind === 'paired' || kind === 'ended') {
			this.pairing.reset();
		}
		// The anchor may have gone meanwhile, as an empty page's buttons do once a computer pairs.
		if (isHideData(data) && anchor.isConnected) {
			(anchor.checkVisibility() ? anchor : anchor.parentElement?.closest<HTMLElement>('[tabindex]'))?.focus();
		}
	}
}

/** Formats the time left on an invitation as `m:ss`. */
function formatRemaining(milliseconds: number): string {
	const seconds = Math.max(0, Math.ceil(milliseconds / 1000));
	return `${String(Math.floor(seconds / 60))}:${String(seconds % 60).padStart(2, '0')}`;
}

/** One button in the popover's foot. */
interface IPairingCommand {
	readonly label: string;
	readonly run: () => void;
	/** Whether it waits for something the person has not done yet, such as typing the whole code. */
	readonly needsCode?: boolean;
}

/** What the popover shows for one pairing step. */
interface IPairingView {
	/** Where this step sits in its flow, as `[step, steps]`; omitted for an outcome. */
	readonly progress?: readonly [number, number];
	readonly heading: string;
	readonly help?: string;
	/** Digits this computer shows for the other person to read. */
	readonly code?: string;
	readonly fileName?: string;
	readonly askCode?: boolean;
	/** Whether a dropped invitation file opens, as choosing one does. */
	readonly acceptsFile?: boolean;
	readonly waiting?: string;
	/** Hides the service's message where the heading and help already say it. */
	readonly quiet?: boolean;
	readonly primary?: IPairingCommand;
	readonly secondary?: IPairingCommand;
}

/** The popover's body for one showing; it follows the pairing state until hidden. */
class PairingPopoverContent extends Disposable {
	private static nextId = 0;
	private readonly root: HTMLElement;
	private readonly progress: HTMLElement;
	private readonly heading: HTMLElement;
	private readonly help: HTMLElement;
	private readonly file: HTMLElement;
	private readonly fileName: HTMLElement;
	private readonly changeFile: HTMLButtonElement;
	private readonly code: HTMLElement;
	private readonly field: HTMLElement;
	private readonly waiting: HTMLElement;
	private readonly expiry: HTMLElement;
	private readonly message: HTMLElement;
	private readonly digits: CodeInput;
	private readonly primary: Button;
	private readonly secondary: Button;
	private readonly countdown = this._register(new IntervalTimer());
	private view: IPairingView | undefined;

	private get acceptsFile(): boolean {
		return !!this.view?.acceptsFile && !this.pairing.state.busy && this.devices.available;
	}

	private shownStep: string | undefined;

	constructor(
		container: HTMLElement,
		private readonly kind: PairingPopoverKind,
		private readonly pairing: IDevicePairingService,
		private readonly devices: IDeviceService,
		private readonly close: () => void,
	) {
		super();
		const headingId = `device-pairing-heading-${String(PairingPopoverContent.nextId++)}`;
		const popover = h('section.device-pairing-popover', { role: 'dialog', 'aria-labelledby': headingId }, [
			h('h2@heading', { id: headingId }),
			h('p.device-pairing-help@help'),
			h('div.device-pairing-file@file', [h('span.device-pairing-file-name@fileName'), h('button.device-pairing-change@changeFile', { type: 'button' })]),
			h('output.device-pairing-code@code', { 'aria-label': nls.localize('devices.code', 'Confirmation code') }),
			h('div.device-pairing-field@field'),
			h('p.device-feedback@message', { role: 'status' }),
			h('p.device-pairing-waiting@waiting'),
			h('div.device-pairing-foot@actions', [h('span.device-pairing-status', [h('span.device-pairing-progress@progress'), h('span.device-pairing-expiry@expiry')])]),
		]);
		this.root = popover.root;
		this.progress = popover.progress;
		this.heading = popover.heading;
		this.help = popover.help;
		this.file = popover.file;
		this.fileName = popover.fileName;
		this.changeFile = popover.changeFile;
		this.code = popover.code;
		this.field = popover.field;
		this.waiting = popover.waiting;
		this.expiry = popover.expiry;
		this.message = popover.message;
		this.changeFile.textContent = nls.localize('devices.changeInvitation', 'Change…');
		this.digits = this._register(new CodeInput(this.field, {
			...defaultCodeInputStyles,
			length: 3,
			label: nls.localize('devices.enterCode', 'Code shown on the other computer'),
			digitLabel: (position, length) => nls.localize('devices.codeDigit', 'Digit {0} of {1}', position, length),
		}));
		this.secondary = this.createButton(popover.actions, false);
		this.primary = this.createButton(popover.actions, true);
		container.append(this.root);
		this.registerListeners();
		this.render(this.pairing.state);
		this._register(toDisposable(() => {
			this.root.remove();
		}));
	}

	private createButton(container: HTMLElement, primary: boolean): Button {
		const button = this._register(new Button(container, { ...defaultButtonStyles, secondary: !primary, title: false }));
		button.element.classList.add('device-button', ...(primary ? ['primary'] : []));
		return button;
	}

	private registerListeners(): void {
		this._register(this.pairing.onDidChange(state => {
			this.render(state);
		}));
		this._register(this.devices.onDidChangeState(() => {
			this.render(this.pairing.state);
		}));
		this._register(this.primary.onDidClick(() => this.view?.primary?.run()));
		this._register(this.secondary.onDidClick(() => this.view?.secondary?.run()));
		this._register(this.digits.onDidChange(() => {
			this.renderButtons(this.pairing.state.busy);
		}));
		// Enter submits even an incomplete code, so the service can say what is missing.
		this._register(this.digits.onDidSubmit(() => {
			if (!this.pairing.state.busy) {
				this.view?.primary?.run();
			}
		}));
		this._register(addDisposableListener(this.changeFile, EventType.CLICK, () => void this.chooseInvitation()));
		// The whole popover takes a dropped invitation while one is wanted.
		this._register(addDisposableListener(this.root, EventType.DRAG_OVER, (event: DragEvent) => {
			if (this.acceptsFile && event.dataTransfer?.types.includes('Files')) {
				event.preventDefault();
				event.dataTransfer.dropEffect = 'copy';
				this.root.classList.add('dropping');
			}
		}));
		this._register(addDisposableListener(this.root, EventType.DRAG_LEAVE, (event: DragEvent) => {
			if (!(event.relatedTarget instanceof Node && this.root.contains(event.relatedTarget))) {
				this.root.classList.remove('dropping');
			}
		}));
		this._register(addDisposableListener(this.root, EventType.DROP, (event: DragEvent) => {
			this.root.classList.remove('dropping');
			const file = event.dataTransfer?.files[0];
			if (file && this.acceptsFile) {
				event.preventDefault();
				void this.pairing.openInvitation(file);
			}
		}));
	}

	public focus(): void {
		if (!this.field.hidden) {
			this.digits.focus();
		} else if (!this.primary.element.hidden) {
			this.primary.focus();
		} else {
			this.root.tabIndex = -1;
			this.root.focus();
		}
	}

	/** Each step of both flows: inviting from here (two steps) or opening an invitation (three). */
	private describe(state: PairingState): IPairingView {
		const { step, busy } = state;
		const done: IPairingCommand = { label: nls.localize('devices.done', 'Done'), run: this.close };
		const choose: IPairingCommand = { label: nls.localize('devices.chooseInvitation', 'Choose invitation…'), run: () => void this.chooseInvitation() };
		const submit = (label: string): IPairingCommand => ({ label, needsCode: true, run: () => void this.pairing.submitCode(this.digits.value) });
		// Opening an invitation replaces one this computer is still offering.
		const shown = this.kind === 'import' && step.kind === 'invitation' ? { kind: 'idle' as const } : step;
		switch (shown.kind) {
			case 'invitation':
				return {
					progress: [1, 2],
					heading: nls.localize('devices.shareTitle', 'Invite another computer'),
					help: nls.localize('devices.shareHelp', 'Save the .vnl file and send it to the other person, then read them these digits. Keep VniLink open.'),
					code: shown.file.check,
					primary: {
						label: nls.localize('devices.save', 'Save…'), run: () => {
							triggerDownload(new Uint8Array(shown.file.bytes), 'VniLink.vnl');
						}
					},
				};
			case 'approval':
				return {
					progress: [2, 2],
					heading: nls.localize('devices.approveTitle', 'Approve this computer'),
					help: nls.localize('devices.approveHelp', 'Ask the other person for the three digits on their screen before pairing.'),
					askCode: true,
					primary: submit(busy ? nls.localize('devices.approving', 'Approving…') : nls.localize('devices.approve', 'Approve')),
					secondary: { label: nls.localize('devices.reject', 'Reject'), run: () => void this.pairing.reject() },
				};
			case 'opened':
				return {
					progress: [2, 3],
					heading: nls.localize('devices.enterCodeTitle', 'Enter their code'),
					help: nls.localize('devices.importHelp', 'Ask the person who created this invitation for the three digits shown on their screen.'),
					fileName: shown.fileName,
					askCode: true,
					acceptsFile: true,
					primary: submit(busy ? nls.localize('devices.pairing', 'Pairing…') : nls.localize('devices.pair', 'Pair')),
				};
			case 'reverseCode':
				return {
					progress: [3, 3],
					heading: nls.localize('devices.reverseTitle', 'Confirm on the other computer'),
					help: nls.localize('devices.reverseCheck', 'Read these digits to the other person so they can approve this computer.'),
					code: shown.code,
					waiting: nls.localize('devices.waitingApproval', 'Waiting for them to approve this computer…'),
					primary: { ...done, label: nls.localize('devices.close', 'Close') },
				};
			case 'paired':
				return {
					heading: nls.localize('devices.pairedTitle', 'Computer paired'),
					help: nls.localize('devices.pairedHelp', 'It is in your devices now. Your sharing settings still control what it can access.'),
					quiet: true,
					primary: done,
				};
			case 'ended':
				return this.kind === 'create'
					? { heading: nls.localize('devices.endedInvitation', 'Invitation ended'), primary: { label: nls.localize('devices.newInvitation', 'Create new invitation'), run: () => void this.pairing.createInvitation() } }
					: { heading: nls.localize('devices.endedPairing', 'Pairing ended'), acceptsFile: true, primary: choose };
			case 'idle':
				return this.kind === 'import'
					? {
						progress: [1, 3],
						heading: nls.localize('devices.importTitle', 'Open an invitation'),
						help: nls.localize('devices.chooseHelp', 'Choose the .vnl file the other computer saved for you, or drop it here.'),
						acceptsFile: true,
						primary: choose,
					}
					: {
						progress: [1, 2],
						heading: nls.localize('devices.shareTitle', 'Invite another computer'),
						help: busy ? nls.localize('devices.creating', 'Creating an invitation…') : undefined,
					};
		}
	}

	private render(state: PairingState): void {
		const { busy, message } = state;
		const view = this.describe(state);
		this.view = view;
		const askChanged = this.field.hidden && !!view.askCode;
		this.countdown.cancel();
		const step = this.pairing.state.step;
		if (step.kind === 'invitation' && this.kind === 'create') {
			this.countdown.cancelAndSet(() => {
				this.renderExpiry(step.file.expiresAt);
			}, 1000, getWindow(this.root));
		}
		this.root.dataset.step = this.shownKind(state);
		this.progress.textContent = view.progress ? nls.localize('devices.stepOf', '{0} of {1}', view.progress[0], view.progress[1]) : '';
		this.progress.hidden = !view.progress;
		this.heading.textContent = view.heading;
		this.help.textContent = view.help ?? '';
		this.help.hidden = !view.help;
		this.fileName.textContent = view.fileName ?? '';
		this.file.hidden = !view.fileName;
		this.changeFile.disabled = busy;
		this.code.textContent = view.code ?? '';
		this.code.hidden = !view.code;
		this.field.hidden = !view.askCode;
		this.waiting.textContent = view.waiting ?? '';
		this.waiting.hidden = !view.waiting;
		this.renderExpiry(step.kind === 'invitation' && this.kind === 'create' ? step.file.expiresAt : 0);
		this.message.textContent = view.quiet ? '' : message;
		// A message beside the code is what went wrong with it.
		this.message.classList.toggle('error', !!view.askCode && !!message);
		this.renderButtons(busy);
		this.root.setAttribute('aria-busy', String(busy));
		this.digits.enabled = !busy;
		const shownStep = this.shownKind(state);
		const stepChanged = this.shownStep !== undefined && this.shownStep !== shownStep;
		this.shownStep = shownStep;
		if (askChanged) {
			this.digits.clear();
		}
		if (stepChanged && !busy && isAncestor(getWindow(this.root).document.activeElement, this.root)) {
			this.focus();
		} else if (!busy && view.askCode && message && this.root.contains(getWindow(this.root).document.activeElement)) {
			// A refused code is typed again from the start.
			this.digits.focus();
		}
	}

	private shownKind(state: PairingState): string {
		return this.kind === 'import' && state.step.kind === 'invitation' ? 'idle' : state.step.kind;
	}

	private renderButtons(busy: boolean): void {
		this.renderButton(this.primary, this.view?.primary, busy);
		this.renderButton(this.secondary, this.view?.secondary, busy);
	}

	private renderButton(button: Button, command: IPairingCommand | undefined, busy: boolean): void {
		button.element.hidden = !command;
		button.label = command?.label ?? '';
		button.enabled = !busy && this.devices.available && (!command?.needsCode || this.digits.complete);
	}

	private renderExpiry(expiresAt: number): void {
		const remaining = expiresAt - Date.now();
		this.expiry.hidden = !expiresAt || remaining <= 0;
		this.expiry.textContent = this.expiry.hidden ? '' : nls.localize('devices.expiresIn', 'Expires in {0}', formatRemaining(remaining));
	}

	private async chooseInvitation(): Promise<void> {
		// The chooser never reports a cancel, so nothing waits on it beyond this call.
		const file = (await triggerUpload())?.[0];
		if (file) {
			await this.pairing.openInvitation(file);
		}
	}
}
