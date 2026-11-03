// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'assert';
import { timeout } from '../../../../../base/common/async.js';
import { Event } from '../../../../../base/common/event.js';
import { ensureNoDisposablesAreLeakedInTestSuite } from '../../../../../base/test/common/utils.js';
import { TestInstantiationService } from '../../../../../platform/instantiation/test/common/instantiationServiceMock.js';
import type { UIStateKey, UIStateValues } from '../../../../../platform/state/common/uiState.js';
import { IApplicationLocationService } from '../../../../services/applicationLocation/applicationLocationService.js';
import { IWorkbenchConfigurationService } from '../../../../services/configuration/configurationService.js';
import { TestConfigurationService } from '../../../../services/configuration/test/common/testConfigurationService.js';
import { IDeviceService } from '../../../../services/devices/deviceService.js';
import { TestDeviceService } from '../../../../services/devices/test/common/testDeviceService.js';
import { IHomeService } from '../../../../services/home/homeService.js';
import { IPermissionService } from '../../../../services/permissions/permissionService.js';
import { TestPermissionService } from '../../../../services/permissions/test/common/testPermissionService.js';
import { ITabService } from '../../../../services/tabs/tabService.js';
import { IUIStateService } from '../../../../services/uiState/uiStateService.js';
import { neededPermissions, onboardingSettings, onboardingSteps, sharingSettings, type OnboardingChoices } from '../../onboardingModel.js';
import { OnboardingView } from '../../onboardingView.js';

const nothingShared = Object.fromEntries(sharingSettings.map(key => [key, false])) as OnboardingChoices['sharing'];

const choices = (overrides: Partial<OnboardingChoices>): OnboardingChoices => ({
	computerName: '', role: 'both', sharing: nothingShared, unattended: false, openAtLogin: false, runInBackground: true, ...overrides,
});

suite('onboarding model', () => {
	ensureNoDisposablesAreLeakedInTestSuite();
	test('a computer only for controlling is asked nothing about sharing or permissions', () => {
		assert.deepStrictEqual(onboardingSteps('control', []), ['welcome', 'name', 'role', 'done']);
	});
	test('a reachable computer is asked what to share, and for permissions only when some are needed', () => {
		assert.deepStrictEqual(onboardingSteps('reachable', ['screen']), ['welcome', 'name', 'role', 'access', 'permissions', 'done']);
		assert.deepStrictEqual(onboardingSteps('both', []), ['welcome', 'name', 'role', 'access', 'done']);
	});
	test('sharing the screen needs the screen and Accessibility, where the system asks for them', () => {
		const screen = choices({ sharing: { ...nothingShared, 'sharing.screen': true } });
		assert.deepStrictEqual(neededPermissions(screen, ['screen', 'accessibility', 'camera']), ['screen', 'accessibility']);
		assert.deepStrictEqual(neededPermissions(screen, ['camera']), []);
		assert.deepStrictEqual(neededPermissions(choices({ role: 'control', sharing: { ...nothingShared, 'sharing.screen': true } }), ['screen']), []);
	});
	test('controlling only accepts no connections and leaves sharing as it was', () => {
		assert.deepStrictEqual(onboardingSettings(choices({ role: 'control', computerName: ' Studio ' }), () => true), [
			['application.computerName', 'Studio'], ['application.acceptConnections', false],
		]);
	});
	test('a reachable computer saves what it shares, whether it asks, and how it stays reachable, except what it cannot change', () => {
		const keys = onboardingSettings(choices({ role: 'reachable' }), key => key !== 'application.openAtLogin').map(([key]) => key);
		assert.deepStrictEqual(keys, ['application.computerName', 'application.acceptConnections', ...sharingSettings, 'sharing.unattended', 'application.runInBackground']);
		assert.deepStrictEqual(onboardingSettings(choices({ role: 'reachable', unattended: true }), () => true).find(([key]) => key === 'sharing.unattended'), ['sharing.unattended', true]);
	});
});

suite('OnboardingView', () => {
	const disposables = ensureNoDisposablesAreLeakedInTestSuite();
	let configuration: TestConfigurationService;
	let permissions: TestPermissionService;
	let view: OnboardingView;
	let saved: Record<string, unknown>;
	let shown: string[];
	let closed: string[];
	setup(async () => {
		configuration = disposables.add(new TestConfigurationService());
		permissions = disposables.add(new TestPermissionService());
		saved = {};
		shown = [];
		closed = [];
		const instantiationService = disposables.add(new TestInstantiationService());
		instantiationService.stub(IWorkbenchConfigurationService, configuration);
		instantiationService.stub(IPermissionService, permissions);
		instantiationService.stub(IDeviceService, disposables.add(new TestDeviceService()));
		instantiationService.stub(IApplicationLocationService, { getLocation: () => Promise.resolve({ movable: false }), move: () => Promise.resolve(false) });
		instantiationService.stub(IHomeService, { requestedPage: undefined, onDidRequestPage: Event.None, showPage: (id: string) => void shown.push(id) });
		instantiationService.stub(ITabService, {
			tabs: [], activeTab: undefined, onDidChange: Event.None, open: () => undefined, select: () => undefined, move: () => undefined,
			close: (id: string) => Promise.resolve(closed.push(id) > 0),
		});
		const uiState: IUIStateService = {
			_serviceBrand: undefined,
			get: <K extends UIStateKey>(key: K) => Promise.resolve(saved[key] as UIStateValues[K] | undefined),
			set: (key, value) => {
				saved[key] = value;
				return Promise.resolve();
			},
		};
		instantiationService.stub(IUIStateService, uiState);
		view = disposables.add(instantiationService.createInstance(OnboardingView));
		document.body.append(view.element);
		await timeout(0);
	});
	teardown(() => {
		view.element.remove();
	});
	const heading = (): string => view.element.querySelector('h1')?.textContent ?? '';
	const progress = (): string => view.element.querySelector('.onboarding-progress')?.textContent ?? '';
	const buttons = (): HTMLElement[] => [...view.element.querySelectorAll<HTMLElement>('.onboarding-actions .monaco-button')].filter(button => !button.hidden);
	const press = async (label: string): Promise<void> => {
		const button = buttons().find(candidate => candidate.textContent === label);
		assert.ok(button, `${label} is offered; ${buttons().map(candidate => candidate.textContent).join(', ')} are`);
		button.dispatchEvent(new MouseEvent('click', { bubbles: true, cancelable: true }));
		await timeout(0);
	};
	const choose = async (selector: string): Promise<void> => {
		const input = view.element.querySelector<HTMLInputElement>(selector);
		assert.ok(input, `${selector} is shown`);
		// As a click on a radio button or switch does: it takes the new state, then says so.
		input.checked = input.type === 'radio' ? true : !input.checked;
		input.dispatchEvent(new window.Event('change', { bubbles: true }));
		await timeout(0);
	};
	test('opens on the welcome, with the way to skip and no way back', () => {
		assert.strictEqual(heading(), 'Welcome to VniLink');
		assert.deepStrictEqual(buttons().map(button => button.textContent), ['Skip setup', 'Get Started']);
	});
	test('each step moves focus to its heading and says where it is', async () => {
		await press('Get Started');
		assert.strictEqual(heading(), 'Name this computer');
		assert.strictEqual(document.activeElement, view.element.querySelector('h1'));
		assert.match(progress(), /^Step 2 of \d+$/);
	});
	test('controlling only reaches the end in four steps and accepts no connections', async () => {
		await press('Get Started');
		await press('Continue');
		await choose('input[value="control"]');
		await press('Continue');
		assert.strictEqual(heading(), 'VniLink is ready');
		assert.strictEqual(progress(), 'Step 4 of 4');
		await press('Go to Devices');
		assert.strictEqual(configuration.getValue('application.acceptConnections'), false);
		assert.deepStrictEqual([saved['onboarding.completed'], shown, closed], [true, ['devices'], ['onboarding']]);
	});
	test('sharing the screen asks for the screen permission before the end, and saves the switch', async () => {
		await press('Get Started');
		await press('Continue');
		await choose('input[value="reachable"]');
		await press('Continue');
		assert.strictEqual(heading(), 'What may your paired computers use here?');
		await choose('input[id="onboarding-sharing-screen"]');
		await press('Continue');
		assert.strictEqual(heading(), 'Let VniLink do what you chose');
		assert.deepStrictEqual([...view.element.querySelectorAll('.onboarding-permission .onboarding-choice-title')].map(title => title.textContent), ['Screen Recording']);
		await press('Continue');
		await press('Go to Devices');
		assert.deepStrictEqual([configuration.getValue('sharing.screen'), configuration.getValue('application.acceptConnections')], [true, true]);
	});
	test('skipping saves no choice and does not open the welcome again', async () => {
		await press('Get Started');
		await press('Skip setup');
		assert.deepStrictEqual(configuration.updates, []);
		assert.strictEqual(saved['onboarding.completed'], true);
	});
	test('a choice that cannot be saved keeps the welcome open and says so', async () => {
		configuration.failure = new Error('The engine is stopped.');
		await press('Get Started');
		await press('Continue');
		await choose('input[value="control"]');
		await press('Continue');
		await press('Go to Devices');
		assert.match(view.element.querySelector('.onboarding-status')?.textContent ?? '', /could not be saved/);
		assert.strictEqual(saved['onboarding.completed'], undefined);
	});
});
