// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import './media/onboarding.css';
import * as nls from '../../../nls.js';
import { $, addDisposableListener, append, EventType, h } from '../../../base/browser/dom.js';
import { Button } from '../../../base/browser/ui/button/button.js';
import { InputBox } from '../../../base/browser/ui/inputbox/inputBox.js';
import { Icons } from '../../../base/common/icons.js';
import { Disposable, DisposableStore } from '../../../base/common/lifecycle.js';
import { isMacintosh } from '../../../base/common/platform.js';
import { settingSchema } from '../../../platform/configuration/common/configurationSchema.js';
import { MAX_COMPUTER_NAME_BYTES } from '../../../platform/engine/common/engine.js';
import { permissionActionLabel, permissionDescriptions, permissionNeedsAction, permissionStatuses, permissionTitles } from '../../../platform/permissions/common/permissionText.js';
import type { PermissionCommand, PermissionKind, PermissionSnapshot } from '../../../platform/permissions/common/permissions.js';
import { defaultButtonStyles, defaultInputBoxStyles } from '../../../platform/theme/browser/defaultStyles.js';
import { IApplicationLocationService } from '../../services/applicationLocation/applicationLocationService.js';
import { IWorkbenchConfigurationService } from '../../services/configuration/configurationService.js';
import { IDeviceService } from '../../services/devices/deviceService.js';
import { IHomeService } from '../../services/home/homeService.js';
import { IPermissionService } from '../../services/permissions/permissionService.js';
import type { WorkbenchTab } from '../../services/tabs/tabService.js';
import { ITabService } from '../../services/tabs/tabService.js';
import { IUIStateService } from '../../services/uiState/uiStateService.js';
import { neededPermissions, onboardingSettings, onboardingSteps, reachable, sharingSettings, type OnboardingChoices, type OnboardingRole, type OnboardingStep, type SharingSetting } from './onboardingModel.js';

const roles: readonly (readonly [OnboardingRole, string, string])[] = [
	['control', nls.localize('onboarding.roleControl', 'Use my other computers from this one'), nls.localize('onboarding.roleControlHelp', 'This computer opens the others. Nobody can reach it.')],
	['reachable', nls.localize('onboarding.roleReachable', 'Let my other computers use this one'), nls.localize('onboarding.roleReachableHelp', 'Your paired computers can reach this one, with what you allow next.')],
	['both', nls.localize('onboarding.roleBoth', 'Both'), nls.localize('onboarding.roleBothHelp', 'This computer reaches the others, and they can reach it.')],
];

/**
 * The welcome that sets VniLink up, one step at a time: this computer's name,
 * what it is for, what paired computers may use on it, the permissions that
 * needs, then the way to pairing. Nothing is saved until the last step;
 * skipping saves nothing. Either way the welcome does not open at start again,
 * and Home's General page can open it later.
 */
export class OnboardingView extends Disposable implements WorkbenchTab {
	public static readonly ID = 'onboarding';
	public readonly id = OnboardingView.ID;
	public readonly label = nls.localize('onboarding.tab', 'Welcome');
	public readonly icon = Icons.home;
	/** Setup is drawn on Mica, as Windows 11's own first-run setup is; elsewhere it is a page. */
	public readonly surface = 'material';
	public readonly element: HTMLElement;
	private readonly progress: HTMLElement;
	private readonly heading: HTMLElement;
	private readonly body: HTMLElement;
	private readonly status: HTMLElement;
	private readonly back: Button;
	private readonly next: Button;
	private readonly skip: Button;
	private readonly stepStore = this._register(new DisposableStore());
	private choices: OnboardingChoices;
	private permissions: PermissionSnapshot | undefined;
	private movable = false;
	private step: OnboardingStep = 'welcome';
	private saving = false;

	constructor(
		@IWorkbenchConfigurationService private readonly configuration: IWorkbenchConfigurationService,
		@IPermissionService private readonly permissionService: IPermissionService,
		@IApplicationLocationService private readonly location: IApplicationLocationService,
		@IDeviceService private readonly devices: IDeviceService,
		@IHomeService private readonly homeService: IHomeService,
		@ITabService private readonly tabService: ITabService,
		@IUIStateService private readonly uiState: IUIStateService,
	) {
		super();
		const view = h('main.onboarding', [
			h('section.onboarding-card@card', { 'aria-labelledby': 'onboarding-heading' }, [
				h('p.onboarding-progress@progress', { 'aria-live': 'polite' }),
				h('h1#onboarding-heading@heading', { tabindex: '-1' }),
				h('div.onboarding-body@body'),
				h('p.onboarding-status@status', { role: 'status' }),
				h('div.onboarding-actions@actions', [h('div.onboarding-skip@skipSlot'), h('div.onboarding-steps@stepSlot')]),
			]),
		]);
		this.element = view.root;
		this.progress = view.progress;
		this.heading = view.heading;
		this.body = view.body;
		this.status = view.status;
		this.skip = this._register(new Button(view.skipSlot, { ...defaultButtonStyles, secondary: true, title: false }));
		this.skip.label = nls.localize('onboarding.skip', 'Skip setup');
		this.back = this._register(new Button(view.stepSlot, { ...defaultButtonStyles, secondary: true, title: false }));
		this.back.label = nls.localize('onboarding.back', 'Back');
		this.next = this._register(new Button(view.stepSlot, { ...defaultButtonStyles, title: false }));
		this.choices = this.savedChoices();
		this.registerListeners();
		this.render();
		void this.load();
	}

	private registerListeners(): void {
		this._register(this.next.onDidClick(() => {
			void this.forward();
		}));
		this._register(this.back.onDidClick(() => {
			this.go(-1);
		}));
		this._register(this.skip.onDidClick(() => {
			void this.finish(false);
		}));
		this._register(this.permissionService.onDidFocusWindow(() => {
			// Back from the system's settings: show what changed there.
			if (this.step === 'permissions') {
				void this.runPermission({ type: 'status' });
			}
		}));
	}

	/** The choices as the settings now are, so running the welcome again starts from them. */
	private savedChoices(): OnboardingChoices {
		const value = (key: string): unknown => this.configuration.getValue(key);
		const accepts = value('application.acceptConnections') === true;
		return {
			computerName: typeof value('application.computerName') === 'string' ? String(value('application.computerName')) : '',
			role: accepts ? 'both' : 'control',
			sharing: Object.fromEntries(sharingSettings.map(key => [key, value(key) === true])) as Record<SharingSetting, boolean>,
			unattended: value('sharing.unattended') === true,
			openAtLogin: value('application.openAtLogin') === true,
			runInBackground: value('application.runInBackground') !== false,
		};
	}

	/** What the steps depend on beyond settings: OS permissions and where the application runs. */
	private async load(): Promise<void> {
		const [permissions, location] = await Promise.all([
			this.permissionService.execute({ type: 'status' }).catch(() => undefined),
			this.location.getLocation().catch(() => ({ movable: false })),
		]);
		if (!this.isDisposed()) {
			this.permissions = permissions;
			this.movable = location.movable;
			this.render();
		}
	}

	private get steps(): readonly OnboardingStep[] {
		return onboardingSteps(this.choices.role, this.relevantPermissions());
	}

	/** The permissions the choices need that this system asks for at all. */
	private relevantPermissions(): readonly PermissionKind[] {
		const asked = (this.permissions?.permissions ?? []).filter(permission => permission.status !== 'unsupported' && permission.status !== 'not-required').map(permission => permission.kind);
		return neededPermissions(this.choices, asked);
	}

	private async forward(): Promise<void> {
		if (this.step === 'done') {
			await this.finish(true);
			return;
		}
		this.go(1);
	}

	private go(delta: number): void {
		const steps = this.steps;
		const index = Math.max(0, Math.min(steps.length - 1, steps.indexOf(this.step) + delta));
		this.step = steps[index];
		this.status.textContent = '';
		this.render();
		// The heading says where the person is; focus follows it so a screen reader reads it.
		this.heading.focus();
	}

	private render(): void {
		this.stepStore.clear();
		this.body.replaceChildren();
		const steps = this.steps;
		if (!steps.includes(this.step)) {
			this.step = 'done';
		}
		const index = steps.indexOf(this.step);
		this.progress.textContent = nls.localize('onboarding.progress', 'Step {0} of {1}', index + 1, steps.length);
		this.back.element.hidden = index === 0;
		this.skip.element.hidden = this.step === 'done';
		this.next.enabled = !this.saving;
		switch (this.step) {
			case 'welcome': this.renderWelcome(); break;
			case 'name': this.renderName(); break;
			case 'role': this.renderRole(); break;
			case 'access': this.renderAccess(); break;
			case 'permissions': this.renderPermissions(); break;
			case 'done': this.renderDone(); break;
		}
	}

	private renderWelcome(): void {
		this.heading.textContent = nls.localize('onboarding.welcomeTitle', 'Welcome to VniLink');
		append(this.body, $('p')).textContent = nls.localize('onboarding.welcomeText', 'VniLink connects your computers, so you can use one from another: its screen, files, terminal, camera and sound. A few choices set it up; you can change each of them later in Settings.');
		this.next.label = nls.localize('onboarding.start', 'Get Started');
	}

	private renderName(): void {
		this.heading.textContent = nls.localize('onboarding.nameTitle', 'Name this computer');
		const help = append(this.body, $('p#onboarding-name-help'));
		help.textContent = nls.localize('onboarding.nameText', 'Your paired computers show this name. Leave it empty to use the name from your system settings.');
		const box = this.stepStore.add(new InputBox(this.body, undefined, {
			inputBoxStyles: defaultInputBoxStyles,
			ariaLabel: nls.localize('onboarding.nameLabel', 'Computer name'),
			validationOptions: {
				validation: value => new TextEncoder().encode(value.trim()).length > MAX_COMPUTER_NAME_BYTES
					? { content: nls.localize('onboarding.nameTooLong', 'Use a shorter name.') }
					: null,
			},
		}));
		box.inputElement.setAttribute('aria-describedby', 'onboarding-name-help');
		box.value = this.choices.computerName;
		this.stepStore.add(box.onDidChange(value => {
			this.choices = { ...this.choices, computerName: value };
			this.next.enabled = box.validate() === undefined;
		}));
		void this.devices.systemComputerName().then(name => {
			if (!box.inputElement.isConnected || !name) {
				return;
			}
			box.setPlaceHolder(name);
		}, () => undefined);
		this.next.label = nls.localize('onboarding.next', 'Continue');
	}

	private renderRole(): void {
		this.heading.textContent = nls.localize('onboarding.roleTitle', 'How will you use this computer?');
		const group = append(this.body, $('div.onboarding-choices', { role: 'radiogroup', 'aria-labelledby': 'onboarding-heading' }));
		for (const [role, title, help] of roles) {
			const id = `onboarding-role-${role}`;
			const option = append(group, $('label.onboarding-choice', { for: id }));
			const radio = append(option, $<HTMLInputElement>('input', { type: 'radio', name: 'onboarding-role', id, value: role }));
			radio.checked = this.choices.role === role;
			const text = append(option, $('span.onboarding-choice-text'));
			append(text, $('span.onboarding-choice-title')).textContent = title;
			append(text, $('span.onboarding-choice-help')).textContent = help;
			this.stepStore.add(addDisposableListener(radio, EventType.CHANGE, () => {
				if (radio.checked) {
					this.choices = { ...this.choices, role };
					this.progress.textContent = nls.localize('onboarding.progress', 'Step {0} of {1}', this.steps.indexOf(this.step) + 1, this.steps.length);
				}
			}));
		}
		this.next.label = nls.localize('onboarding.next', 'Continue');
	}

	private renderAccess(): void {
		this.heading.textContent = nls.localize('onboarding.accessTitle', 'What may your paired computers use here?');
		append(this.body, $('p')).textContent = nls.localize('onboarding.accessText', 'A computer you pair can use what you turn on. Turning something off later ends its use right away.');
		const list = append(this.body, $('div.onboarding-switches'));
		for (const key of sharingSettings) {
			this.renderSwitch(list, key, this.choices.sharing[key], checked => {
				this.choices = { ...this.choices, sharing: { ...this.choices.sharing, [key]: checked } };
			});
		}
		append(this.body, $('h2')).textContent = nls.localize('onboarding.acceptingTitle', 'Accepting computers');
		const accepting = append(this.body, $('div.onboarding-switches'));
		this.renderSwitch(accepting, 'sharing.unattended', this.choices.unattended, checked => {
			this.choices = { ...this.choices, unattended: checked };
		});
		append(this.body, $('h2')).textContent = nls.localize('onboarding.availabilityTitle', 'Staying reachable');
		const availability = append(this.body, $('div.onboarding-switches'));
		this.renderSwitch(availability, 'application.openAtLogin', this.choices.openAtLogin, checked => {
			this.choices = { ...this.choices, openAtLogin: checked };
		});
		this.renderSwitch(availability, 'application.runInBackground', this.choices.runInBackground, checked => {
			this.choices = { ...this.choices, runInBackground: checked };
		});
		this.next.label = nls.localize('onboarding.next', 'Continue');
	}

	/** One labelled switch for `key`, disabled with the reason where this launch cannot change it. */
	private renderSwitch(container: HTMLElement, key: string, checked: boolean, onChange: (checked: boolean) => void): void {
		const schema = settingSchema(key);
		const id = `onboarding-${key.replace(/\W/g, '-')}`;
		const row = append(container, $('label.onboarding-switch', { for: id }));
		const text = append(row, $('span.onboarding-choice-text'));
		append(text, $('span.onboarding-choice-title')).textContent = schema?.title ?? key;
		const restricted = this.configuration.isRestricted(key);
		const reason = restricted ? schema?.restrictedReason : schema?.description;
		if (reason) {
			append(text, $('span.onboarding-choice-help', { id: `${id}-help` })).textContent = reason;
		}
		const input = append(row, $<HTMLInputElement>('input.setting-switch', { type: 'checkbox', role: 'switch', id }));
		input.checked = checked && !restricted;
		input.disabled = restricted;
		if (reason) {
			input.setAttribute('aria-describedby', `${id}-help`);
		}
		this.stepStore.add(addDisposableListener(input, EventType.CHANGE, () => {
			onChange(input.checked);
		}));
	}

	private renderPermissions(): void {
		this.heading.textContent = nls.localize('onboarding.permissionsTitle', 'Let VniLink do what you chose');
		append(this.body, $('p')).textContent = isMacintosh
			? nls.localize('onboarding.permissionsTextMac', 'macOS asks you to allow these once. Changes made in System Settings may need VniLink to restart.')
			: nls.localize('onboarding.permissionsText', 'Your system asks you to allow these once.');
		if (this.movable) {
			const move = append(this.body, $('div.onboarding-note'));
			append(move, $('p')).textContent = nls.localize('onboarding.moveText', 'VniLink isn’t in the Applications folder. macOS keeps these permissions only while VniLink stays in one place, so move it first; it restarts from there.');
			const button = this.stepStore.add(new Button(move, { ...defaultButtonStyles, secondary: true, title: false }));
			button.label = nls.localize('onboarding.move', 'Move to Applications');
			this.stepStore.add(button.onDidClick(() => {
				void this.location.move().then(moved => {
					if (!moved && !this.isDisposed()) {
						this.status.textContent = nls.localize('onboarding.moveFailed', 'VniLink could not move itself. Drag it to the Applications folder in Finder.');
					}
				});
			}));
		}
		const list = append(this.body, $('div.onboarding-switches'));
		for (const kind of this.relevantPermissions()) {
			const permission = this.permissions?.permissions.find(candidate => candidate.kind === kind);
			if (!permission) {
				continue;
			}
			const row = append(list, $('div.onboarding-permission'));
			const text = append(row, $('span.onboarding-choice-text'));
			append(text, $('span.onboarding-choice-title')).textContent = permissionTitles[kind];
			const help = permissionDescriptions[kind];
			if (help) {
				append(text, $('span.onboarding-choice-help')).textContent = help;
			}
			append(row, $('span.setting-status', { 'data-status': permission.status })).textContent = permissionStatuses[permission.status];
			if (permissionNeedsAction(permission)) {
				const button = this.stepStore.add(new Button(row, { ...defaultButtonStyles, secondary: true, title: false }));
				button.label = permissionActionLabel(permission);
				button.element.setAttribute('aria-label', nls.localize('permissions.actionLabel', '{0}: {1}', permissionTitles[kind], permissionActionLabel(permission)));
				this.stepStore.add(button.onDidClick(() => {
					void this.runPermission({ type: permission.canRequest ? 'request' : 'openSettings', kind });
				}));
			}
		}
		this.next.label = nls.localize('onboarding.next', 'Continue');
	}

	private async runPermission(command: PermissionCommand): Promise<void> {
		try {
			const snapshot = await this.permissionService.execute(command);
			if (!this.isDisposed()) {
				this.permissions = snapshot;
				this.render();
			}
		} catch {
			if (!this.isDisposed()) {
				this.status.textContent = nls.localize('permissions.failed', 'Could not check or change OS access. The status will be checked again automatically.');
			}
		}
	}

	private renderDone(): void {
		this.heading.textContent = nls.localize('onboarding.doneTitle', 'VniLink is ready');
		append(this.body, $('p')).textContent = reachable(this.choices.role)
			? nls.localize('onboarding.doneReachable', 'Next, pair your other computers: on Devices, invite one, or open the invitation it sent you. A paired computer can use only what you turned on.')
			: nls.localize('onboarding.doneControl', 'Next, pair your other computers: on Devices, open the invitation one sent you, or invite one.');
		this.next.label = nls.localize('onboarding.goToDevices', 'Go to Devices');
	}

	/** Saves the choices when `save`, then closes the welcome for good and shows Devices. */
	private async finish(save: boolean): Promise<void> {
		if (this.saving) {
			return;
		}
		if (save) {
			this.saving = true;
			this.next.enabled = false;
			try {
				const writable = (key: string): boolean => !this.configuration.isRestricted(key) && this.configuration.getUnavailableReason(key) === undefined;
				for (const [key, value] of onboardingSettings(this.choices, writable)) {
					if (this.configuration.getValue(key) !== value) {
						await this.configuration.updateValue(key, value);
					}
				}
			} catch (error) {
				if (!this.isDisposed()) {
					this.saving = false;
					this.next.enabled = true;
					this.status.textContent = nls.localize('onboarding.saveFailed', 'Some choices could not be saved: {0} You can change them in Settings.', error instanceof Error ? error.message : String(error));
				}
				return;
			}
		}
		await this.uiState.set('onboarding.completed', true).catch(() => undefined);
		if (!this.isDisposed()) {
			this.homeService.showPage('devices');
			void this.tabService.close(this.id);
		}
	}

	private isDisposed(): boolean {
		return this._store.isDisposed;
	}
}
