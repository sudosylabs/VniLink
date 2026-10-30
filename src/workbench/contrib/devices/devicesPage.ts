// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import './media/devices.css';
import * as nls from '../../../nls.js';
import { addDisposableListener, append, EventType, h } from '../../../base/browser/dom.js';
import { renderIcon } from '../../../base/browser/ui/icon/icon.js';
import { Icons, type IconId } from '../../../base/common/icons.js';
import { Disposable } from '../../../base/common/lifecycle.js';
import { ICommandService } from '../../../platform/commands/common/commands.js';
import { IContextKeyService } from '../../../platform/contextkey/common/contextkey.js';
import { IInstantiationService } from '../../../platform/instantiation/common/instantiation.js';
import { EngineAvailableContext } from '../../contextkeys.js';
import { IDeviceService } from '../../services/devices/deviceService.js';
import { createPageHeading, type IHomePage } from '../home/homePages.js';
import { CreateInvitationAction, ImportInvitationAction } from './deviceActions.js';
import { DeviceDetails } from './deviceDetails.js';
import { DeviceList } from './deviceList.js';

interface IPairingChoice {
	readonly command: string;
	readonly icon: IconId;
	readonly label: string;
}

/**
 * Home's Devices page: remembered computers beside the selected one's
 * details. Pairing starts from the page: two quiet buttons under the heading
 * once computers are listed, the empty page's own two buttons before that.
 */
export class DevicesPage extends Disposable implements IHomePage {
	private static nextId = 0;
	public readonly element: HTMLElement;
	/** The list and the inspector scroll on their own, so the inspector stays a full-height sheet. */
	public readonly scrollsItself = true;
	private readonly main: HTMLElement;
	private readonly empty: HTMLElement;
	private readonly emptyTitle: HTMLElement;
	private readonly emptyHelp: HTMLElement;
	private readonly feedback: HTMLElement;
	private readonly headingActions: HTMLElement;
	private readonly emptyChoices: HTMLElement;
	private readonly pairingButtons: HTMLButtonElement[] = [];
	private readonly list: DeviceList;
	private readonly details: DeviceDetails;
	private selectedId: string | undefined;

	constructor(
		@IDeviceService private readonly devices: IDeviceService,
		@ICommandService private readonly commandService: ICommandService,
		@IContextKeyService private readonly contextKeyService: IContextKeyService,
		@IInstantiationService instantiationService: IInstantiationService,
	) {
		super();
		const detailsId = `device-details-${String(DevicesPage.nextId++)}`;
		this.list = this._register(new DeviceList(detailsId));
		this.details = this._register(instantiationService.createInstance(DeviceDetails, detailsId));
		// The page is the size container; the grid inside it can then change with the width.
		const page = h('main.devices-page', { tabindex: '-1' }, [
			h('div.devices-layout@layout', [
				h('div.devices-main@main', [
					h('p.device-feedback@feedback', { role: 'status' }),
					h('div.devices-heading@heading', [h('div.device-pairing-actions@headingActions')]),
					h('div.devices-list-column@column', [
						h('section.device-empty@empty', [
							h('div@emptyStatus', { role: 'status' }, [h('h2@emptyTitle'), h('p@emptyHelp')]),
							h('div.device-pairing-choices@emptyChoices'),
						]),
					]),
				]),
			]),
		]);
		this.element = page.root;
		this.main = page.main;
		this.empty = page.empty;
		this.emptyTitle = page.emptyTitle;
		this.emptyHelp = page.emptyHelp;
		this.feedback = page.feedback;
		this.headingActions = page.headingActions;
		this.emptyChoices = page.emptyChoices;
		this.empty.prepend(renderIcon(Icons.devices));
		page.column.prepend(this.list.element);
		append(page.layout, this.details.element);
		page.heading.prepend(createPageHeading(nls.localize('computers.heading', 'Devices'), nls.localize('computers.subheading', 'Your computers, connected.')));
		this.main.prepend(page.heading);
		this.renderPairingChoices();
		this.registerListeners();
		this.render();
		this.devices.initialize().catch(() => {
			if (!this._store.isDisposed) {
				this.feedback.textContent = nls.localize('devices.loadFailed', 'Unable to load devices. Restart VniLink to try again.');
			}
		});
	}

	private registerListeners(): void {
		this._register(this.list.onDidSelect(device => {
			this.selectedId = device.id;
			this.render();
		}));
		this._register(this.details.onDidClose(() => {
			this.closeDetails();
		}));
		this._register(this.devices.onDidChangeState(() => {
			this.render();
		}));
		this._register(this.devices.onDidChangeDevices(() => {
			this.render();
		}));
		this._register(this.contextKeyService.onDidChangeContext(event => {
			if (event.affectsSome(new Set([EngineAvailableContext.key]))) {
				this.updatePairingEnablement();
			}
		}));
		this._register(addDisposableListener(this.element, EventType.KEY_DOWN, event => {
			if (event.key === 'Escape' && this.selectedId) {
				this.closeDetails();
			}
		}));
	}

	private renderPairingChoices(): void {
		const choices: readonly IPairingChoice[] = [
			{
				command: CreateInvitationAction.ID,
				icon: Icons.createInvitation,
				label: nls.localize('devices.createInvitation', 'Invite another computer'),
			},
			{
				command: ImportInvitationAction.ID,
				icon: Icons.importInvitation,
				label: nls.localize('devices.importInvitation', 'Open an invitation'),
			},
		];
		for (const choice of choices) {
			const compact = h('button.device-pairing-button', { type: 'button' }, [renderIcon(choice.icon), h('span@label')]);
			compact.label.textContent = choice.label;
			// On the empty page inviting leads: the other computer usually has nothing to open yet.
			const choiceButton = h(`button.device-pairing-choice${this.pairingButtons.length ? '' : '.primary'}`, { type: 'button' }).root;
			choiceButton.textContent = choice.label;
			for (const button of [compact.root, choiceButton]) {
				this._register(addDisposableListener(button, EventType.CLICK, () => {
					void this.commandService.executeCommand(choice.command, { anchor: button });
				}));
				this.pairingButtons.push(button);
			}
			this.headingActions.append(compact.root);
			this.emptyChoices.append(choiceButton);
		}
		this.updatePairingEnablement();
	}

	/** The buttons follow the actions' precondition: pairing needs the engine. */
	private updatePairingEnablement(): void {
		const enabled = this.contextKeyService.contextMatchesRules(EngineAvailableContext);
		for (const button of this.pairingButtons) {
			button.disabled = !enabled;
		}
	}

	private closeDetails(): void {
		const id = this.selectedId;
		this.selectedId = undefined;
		this.render();
		this.list.focus(id);
	}

	private render(): void {
		const state = this.devices.state;
		this.emptyTitle.textContent = state === 'loading' ? nls.localize('devices.loading', 'Loading computers…')
			: state === 'unavailable' ? nls.localize('devices.unavailable', 'Devices unavailable')
				: nls.localize('devices.emptyTitle', 'Bring your computers together');
		this.emptyHelp.textContent = state === 'unavailable' ? nls.localize('devices.unavailableHelp', 'Unable to read devices from the connection engine. Restart VniLink to try again.')
			: state === 'loading' ? ''
				: nls.localize('devices.emptyHelp', 'Pair this computer with another to see it here. One of them makes an invitation; the other opens it.');
		const known = this.devices.devices;
		const selected = known.find(device => device.id === this.selectedId);
		const lostFocus = !!this.selectedId && !selected && this.details.element.contains(document.activeElement);
		this.selectedId = selected?.id;
		this.list.update(known, this.selectedId);
		this.details.update(selected);
		this.element.classList.toggle('has-selection', !!selected);
		this.empty.hidden = known.length > 0;
		this.list.element.hidden = !known.length;
		this.headingActions.hidden = !known.length;
		this.emptyChoices.hidden = state !== 'ready';
		if (lostFocus) {
			if (known.length) {
				this.list.focus();
			} else {
				this.element.focus();
			}
		}
	}

	public override dispose(): void {
		super.dispose();
		this.element.remove();
	}
}
