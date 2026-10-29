import './media/devicePairingPopover.css';
import * as nls from '../../../nls.js';
import { addDisposableListener, EventType, getWindow, h, isAncestor, triggerDownload, triggerUpload } from '../../../base/browser/dom.js';
import { StandardKeyboardEvent } from '../../../base/browser/keyboardEvent.js';
import { Button } from '../../../base/browser/ui/button/button.js';
import { ContextView, ContextViewDOMPosition } from '../../../base/browser/ui/contextview/contextview.js';
import { InputBox } from '../../../base/browser/ui/inputbox/inputBox.js';
import { IntervalTimer } from '../../../base/common/async.js';
import { KeyCode } from '../../../base/common/keyCodes.js';
import { AnchorAlignment } from '../../../base/common/layout.js';
import { Disposable, DisposableStore, toDisposable } from '../../../base/common/lifecycle.js';
import { createDecorator } from '../../../platform/instantiation/common/instantiation.js';
import { defaultButtonStyles, defaultInputBoxStyles } from '../../../platform/theme/browser/defaultStyles.js';
import { IDevicePairingService, type PairingState } from '../../services/devices/devicePairingService.js';
import { IDeviceService } from '../../services/devices/deviceService.js';

/** Which titlebar action opened the popover: inviting from here, or opening another computer's invitation. */
export type PairingPopoverKind = 'create' | 'import';

export const IDevicePairingPopover = createDecorator<IDevicePairingPopover>('devicePairingPopover');

/** The pairing popover under the Devices page's titlebar actions. */
export interface IDevicePairingPopover {
	readonly _serviceBrand: undefined;
	/** Shows pairing under `anchor`, or closes it when that action's popover is already open. */
	toggle(anchor: HTMLElement, kind: PairingPopoverKind): void;
	hide(): void;
}

/** Whether a hide returns focus to the titlebar button, as Escape and Done do. */
interface HideData {
	readonly restoreFocus: boolean;
}

function isHideData(value: unknown): value is HideData {
	return typeof value === 'object' && value !== null && 'restoreFocus' in value && value.restoreFocus === true;
}

/**
 * Shows `IDevicePairingService` in a context view anchored to the titlebar
 * button. Closing it leaves a live invitation waiting for the other computer.
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
		if (isHideData(data) && anchor.isConnected) {
			anchor.focus();
		}
	}
}

/** Formats the time left on an invitation as `m:ss`. */
function formatRemaining(milliseconds: number): string {
	const seconds = Math.max(0, Math.ceil(milliseconds / 1000));
	return `${String(Math.floor(seconds / 60))}:${String(seconds % 60).padStart(2, '0')}`;
}

/** The popover's body for one showing; it follows the pairing state until hidden. */
class PairingPopoverContent extends Disposable {
	private static nextId = 0;
	private readonly root: HTMLElement;
	private readonly heading: HTMLElement;
	private readonly help: HTMLElement;
	private readonly code: HTMLElement;
	private readonly expiry: HTMLElement;
	private readonly fileName: HTMLElement;
	private readonly field: HTMLElement;
	private readonly message: HTMLElement;
	private readonly digits: InputBox;
	private readonly primary: Button;
	private readonly secondary: Button;
	private readonly countdown = this._register(new IntervalTimer());
	private primaryRun: (() => void) | undefined;
	private secondaryRun: (() => void) | undefined;
	private shownStep: PairingState['step']['kind'] | undefined;

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
			h('output.device-pairing-code@code', { 'aria-label': nls.localize('devices.code', 'Confirmation code') }),
			h('p.device-pairing-file@fileName'),
			h('label.device-pairing-field@field'),
			h('div.device-pairing-foot@actions', [h('span.device-pairing-expiry@expiry')]),
			h('p.device-feedback@message', { role: 'status' }),
		]);
		this.root = popover.root;
		this.heading = popover.heading;
		this.help = popover.help;
		this.code = popover.code;
		this.expiry = popover.expiry;
		this.fileName = popover.fileName;
		this.field = popover.field;
		this.message = popover.message;
		this.field.append(nls.localize('devices.enterCode', 'Code shown on the other computer'));
		this.digits = this._register(new InputBox(this.field, undefined, { inputBoxStyles: defaultInputBoxStyles }));
		this.digits.inputElement.inputMode = 'numeric';
		this.digits.inputElement.autocomplete = 'off';
		this.digits.inputElement.maxLength = 3;
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
		this._register(this.primary.onDidClick(() => this.primaryRun?.()));
		this._register(this.secondary.onDidClick(() => this.secondaryRun?.()));
		this._register(addDisposableListener(this.digits.inputElement, EventType.KEY_DOWN, event => {
			if (new StandardKeyboardEvent(event).equals(KeyCode.Enter)) {
				event.preventDefault();
				this.primaryRun?.();
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

	private render(state: PairingState): void {
		const { step, busy, message } = state;
		const invite = nls.localize('devices.shareTitle', 'Invite another computer');
		const join = nls.localize('devices.importTitle', 'Pair with an invitation');
		let heading = this.kind === 'create' ? invite : join;
		let help = '';
		let code = '';
		let fileName = '';
		let askDigits = false;
		let primary: { label: string; run: () => void } | undefined;
		let secondary: { label: string; run: () => void } | undefined;
		const choose = { label: nls.localize('devices.chooseInvitation', 'Choose invitation…'), run: () => void this.chooseInvitation() };
		const submit = (label: string) => ({ label, run: () => void this.pairing.submitCode(this.digits.value) });
		const done = { label: nls.localize('devices.done', 'Done'), run: this.close };
		this.countdown.cancel();
		// Opening an invitation replaces one this computer is still offering.
		const shown = this.kind === 'import' && step.kind === 'invitation' ? { kind: 'idle' as const } : step;
		switch (shown.kind) {
			case 'invitation':
				heading = invite;
				help = nls.localize('devices.shareHelp', 'Save the .vnl file and send it to the other person, then read them these digits. Keep VniLink open.');
				code = shown.file.check;
				primary = {
					label: nls.localize('devices.save', 'Save…'), run: () => {
						triggerDownload(new Uint8Array(shown.file.bytes), 'VniLink.vnl');
					}
				};
				this.countdown.cancelAndSet(() => {
					this.renderExpiry(shown.file.expiresAt);
				}, 1000, getWindow(this.root));
				break;
			case 'approval':
				heading = nls.localize('devices.approveTitle', 'Approve this computer');
				help = nls.localize('devices.approveHelp', 'Ask the other person for the three digits on their screen before pairing.');
				askDigits = true;
				primary = submit(nls.localize('devices.approve', 'Approve'));
				secondary = { label: nls.localize('devices.reject', 'Reject'), run: () => void this.pairing.reject() };
				break;
			case 'opened':
				heading = join;
				help = nls.localize('devices.importHelp', 'Ask the person who created this invitation for the three digits shown on their screen.');
				fileName = shown.fileName;
				askDigits = true;
				primary = submit(nls.localize('devices.pair', 'Pair'));
				secondary = { ...choose, label: nls.localize('devices.chooseOther', 'Choose another…') };
				break;
			case 'reverseCode':
				heading = join;
				help = nls.localize('devices.reverseCheck', 'Read these digits to the other person so they can approve this computer.');
				code = shown.code;
				primary = done;
				break;
			case 'paired':
				primary = done;
				break;
			case 'ended':
				primary = this.kind === 'create' ? { label: nls.localize('devices.newInvitation', 'Create new invitation'), run: () => void this.pairing.createInvitation() } : choose;
				break;
			case 'idle':
				if (this.kind === 'import') {
					help = nls.localize('devices.chooseHelp', 'Open the .vnl invitation another computer shared with you.');
					primary = choose;
				} else {
					help = busy ? nls.localize('devices.creating', 'Creating an invitation…') : '';
				}
				break;
		}
		this.heading.textContent = heading;
		this.help.textContent = help;
		this.help.hidden = !help;
		this.code.textContent = code;
		this.code.hidden = !code;
		this.fileName.textContent = fileName;
		this.fileName.hidden = !fileName;
		this.field.hidden = !askDigits;
		this.renderExpiry(shown.kind === 'invitation' ? shown.file.expiresAt : 0);
		this.message.textContent = message;
		this.primaryRun = primary?.run;
		this.secondaryRun = secondary?.run;
		this.renderButton(this.primary, primary?.label, busy);
		this.renderButton(this.secondary, secondary?.label, busy);
		this.root.setAttribute('aria-busy', String(busy));
		if (busy) {
			this.digits.disable();
		} else {
			this.digits.enable();
		}
		const stepChanged = this.shownStep !== undefined && this.shownStep !== shown.kind;
		this.shownStep = shown.kind;
		if (stepChanged && !busy && isAncestor(getWindow(this.root).document.activeElement, this.root)) {
			if (askDigits) {
				this.digits.value = '';
			}
			this.focus();
		}
	}

	private renderButton(button: Button, label: string | undefined, busy: boolean): void {
		button.element.hidden = !label;
		button.label = label ?? '';
		button.enabled = !busy && this.devices.available;
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
