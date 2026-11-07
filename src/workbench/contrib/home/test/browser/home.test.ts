// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'assert';
import { $ } from '../../../../../base/browser/dom.js';
import { timeout } from '../../../../../base/common/async.js';
import { Emitter, Event } from '../../../../../base/common/event.js';
import { toDisposable } from '../../../../../base/common/lifecycle.js';
import { ensureNoDisposablesAreLeakedInTestSuite } from '../../../../../base/test/common/utils.js';
import { IMenuService, type IMenu } from '../../../../../platform/actions/common/actions.js';
import { MenuService } from '../../../../../platform/actions/common/menuService.js';
import { IClipboardService } from '../../../../../platform/clipboard/common/clipboardService.js';
import { ICommandService } from '../../../../../platform/commands/common/commands.js';
import { ContextKeyService } from '../../../../../platform/contextkey/browser/contextKeyService.js';
import { IContextKeyService } from '../../../../../platform/contextkey/common/contextkey.js';
import { TestInstantiationService } from '../../../../../platform/instantiation/test/common/instantiationServiceMock.js';
import { ILogService, NullLogService } from '../../../../../platform/log/common/log.js';
import { INativeHostService } from '../../../../../platform/native/common/native.js';
import '../../../../actions/applicationActions.js';
import { ActiveTabContext, EngineAvailableContext, RestartRequiredContext } from '../../../../contextkeys.js';
import { ViewActionsMenu } from '../../../../parts/titlebar/viewActionsToolbar.js';
import { IApplicationService } from '../../../../services/application/applicationService.js';
import { CommandService } from '../../../../services/commands/commandService.js';
import { IWorkbenchConfigurationService } from '../../../../services/configuration/configurationService.js';
import { TestConfigurationService } from '../../../../services/configuration/test/common/testConfigurationService.js';
import { IConnectionService } from '../../../../services/connection/connectionService.js';
import { IDevicePairingService } from '../../../../services/devices/devicePairingService.js';
import { IDeviceService } from '../../../../services/devices/deviceService.js';
import { IHomeService } from '../../../../services/home/homeService.js';
import { TestDeviceService } from '../../../../services/devices/test/common/testDeviceService.js';
import { IPermissionService } from '../../../../services/permissions/permissionService.js';
import { TestPermissionService } from '../../../../services/permissions/test/common/testPermissionService.js';
import { IScreenSnapshotService } from '../../../../services/screenSnapshot/screenSnapshotService.js';
import { ISystemActionService } from '../../../../services/systemActions/systemActionService.js';
import { TestSystemActionService } from '../../../../services/systemActions/test/common/testSystemActionService.js';
import { TestClipboardService } from '../../../../test/common/workbenchTestServices.js';
import '../../../devices/devices.contribution.js';
import { IDevicePairingPopover, type PairingPopoverKind } from '../../../devices/devicePairingPopover.js';
import '../../../settings/settings.contribution.js';
import { HomeView } from '../../homeView.js';
// Home's content area is styled by the workbench's stylesheet.
import '../../../../media/workbench.css';

/** An {@link IDevicePairingPopover} that keeps each kind it was opened for and the element it was opened at. */
class TestPairingPopover implements IDevicePairingPopover {
	declare public readonly _serviceBrand: undefined;
	public readonly opened: { readonly kind: PairingPopoverKind; readonly anchor: HTMLElement }[] = [];

	public toggle(anchor: HTMLElement, kind: PairingPopoverKind): void {
		this.opened.push({ kind, anchor });
	}

	public hides = 0;
	public readonly isOpen = false;

	public hide(): void {
		this.hides++;
	}
}

/** Clicks `target` as the user's pointer does. */
function click(target: Element): void {
	target.dispatchEvent(new MouseEvent('click', { bubbles: true, cancelable: true }));
}

suite('Home', () => {
	const disposables = ensureNoDisposablesAreLeakedInTestSuite();
	suite('content area', () => {
		let viewport: HTMLElement;
		let page: HTMLElement;
		let last: HTMLElement;
		setup(() => {
			viewport = $('div.home-content');
			viewport.style.cssText = 'position: fixed; top: 0; left: 0; width: 600px; height: 160px;';
			page = $('main');
			last = $('div');
			last.style.height = '600px';
			page.append(last);
			viewport.append(page);
			document.body.append(viewport);
			disposables.add(toDisposable(() => {
				viewport.remove();
			}));
		});
		test('a long page keeps its bottom padding when scrolled to the end', () => {
			viewport.scrollTop = viewport.scrollHeight;
			const bottomGap = viewport.getBoundingClientRect().bottom - last.getBoundingClientRect().bottom;
			assert.ok(bottomGap >= 47, `the gap below the page is ${String(bottomGap)} px`);
		});
		test('a wide page is centred', () => {
			viewport.style.width = '1400px';
			const bounds = page.getBoundingClientRect();
			const container = viewport.getBoundingClientRect();
			const left = bounds.left - container.left;
			const right = container.left + viewport.clientWidth - bounds.right;
			assert.ok(Math.abs(left - right) <= 1, `${String(left)} px on the left, ${String(right)} px on the right`);
		});
	});
	suite('pages', () => {
		let devices: TestDeviceService;
		let popover: TestPairingPopover;
		let restartRequired: boolean;
		let restarts: number;
		let instantiationService: TestInstantiationService;
		let home: HomeView;
		let contextKeys: IContextKeyService;
		let menu: IMenu;
		let pageRequests: Emitter<string>;
		setup(() => {
			pageRequests = disposables.add(new Emitter<string>());
			devices = disposables.add(new TestDeviceService());
			popover = new TestPairingPopover();
			restartRequired = false;
			restarts = 0;
			const configuration = disposables.add(new TestConfigurationService());
			configuration.setError('relays', 'Unavailable');
			instantiationService = disposables.add(new TestInstantiationService(undefined, true, undefined, true));
			instantiationService.stub(IApplicationService, {
				get restartRequired() {
					return restartRequired;
				},
				onDidChangeRestartRequired: disposables.add(new Emitter<void>()).event,
				restart: () => {
					restarts++;
					return Promise.resolve();
				},
				openRelayDocumentation: () => Promise.resolve(),
			});
			instantiationService.stub(IDeviceService, devices);
			instantiationService.stub(IScreenSnapshotService, { capture: () => Promise.resolve({ status: 'disconnected' }), access: () => Promise.resolve({ status: 'disconnected' }) });
			instantiationService.stub(IConnectionService, { getRunningRelays: () => Promise.reject(new Error('Unavailable')) });
			instantiationService.stub(IWorkbenchConfigurationService, configuration);
			instantiationService.stub(IPermissionService, disposables.add(new TestPermissionService()));
			instantiationService.stub(ILogService, new NullLogService());
			instantiationService.stub(IContextKeyService, disposables.add(instantiationService.createInstance(ContextKeyService)));
			instantiationService.stub(ICommandService, disposables.add(instantiationService.createInstance(CommandService)));
			instantiationService.stub(IMenuService, disposables.add(instantiationService.createInstance(MenuService)));
			instantiationService.stub(IDevicePairingPopover, popover);
			instantiationService.stub(IDevicePairingService, { state: { step: { kind: 'idle' }, busy: false, message: '' }, onDidChange: Event.None });
			instantiationService.stub(IHomeService, { requestedPage: undefined, onDidRequestPage: pageRequests.event, showPage: () => undefined });
			instantiationService.stub(ISystemActionService, disposables.add(new TestSystemActionService()));
			instantiationService.stub(IClipboardService, new TestClipboardService());
			instantiationService.stub(INativeHostService, { onDidChangeWindowState: Event.None, setMaterialPanel: () => Promise.resolve() });
			home = disposables.add(instantiationService.createInstance(HomeView));
			contextKeys = instantiationService.get(IContextKeyService);
			menu = disposables.add(instantiationService.get(IMenuService).createMenu(ViewActionsMenu, contextKeys));
		});
		const button = (section: string): HTMLButtonElement => {
			const found = home.element.querySelector<HTMLButtonElement>(`[data-section-id="${section}"]`);
			assert.ok(found, `the ${section} section is missing from the sidebar`);
			return found;
		};
		const shown = (): Element | null => home.element.querySelector('.home-content > main');
		const restartActions = () => menu.getActions().flatMap(([, actions]) => actions).filter(action => action.id === 'workbench.action.restart');
		/** Shows Home with the restart a settings change asked for, or without one. */
		function requireRestart(required: boolean): void {
			ActiveTabContext.bindTo(contextKeys).set(HomeView.ID);
			restartRequired = required;
			RestartRequiredContext.bindTo(contextKeys).set(required);
		}
		test('a page another feature asks for is shown and marked current', () => {
			pageRequests.fire('connection');
			assert.strictEqual(button('connection').getAttribute('aria-current'), 'page');
			assert.strictEqual(button('devices').getAttribute('aria-current'), null);
		});
		test('Files settings do not appear in the sidebar', () => {
			assert.strictEqual(home.element.querySelector('[data-section-id="files"]'), null);
		});
		test('restart is not offered while nothing needs it', () => {
			requireRestart(false);
			assert.strictEqual(restartActions().length, 0);
		});
		test('restart is offered on Home once a change needs it, and restarts through the application service', async () => {
			requireRestart(true);
			assert.strictEqual(restartActions().length, 1);
			await restartActions()[0].run();
			assert.strictEqual(restarts, 1);
		});
		test('restart is not offered outside Home', () => {
			requireRestart(true);
			ActiveTabContext.bindTo(contextKeys).set('remote-session');
			assert.strictEqual(restartActions().length, 0);
		});
		test('restart is withdrawn once the change that needed it is reverted', () => {
			requireRestart(true);
			requireRestart(false);
			assert.strictEqual(restartActions().length, 0);
		});
		test('pairing actions are not in the titlebar', () => {
			ActiveTabContext.bindTo(contextKeys).set(HomeView.ID);
			EngineAvailableContext.bindTo(contextKeys).set(true);
			const pairing = menu.getActions().flatMap(([, actions]) => actions).map(action => action.id).filter(id => id.startsWith('vnilink.devices.'));
			assert.deepStrictEqual(pairing, []);
		});
		test('the empty devices page opens each pairing popover at its button', async () => {
			EngineAvailableContext.bindTo(contextKeys).set(true);
			const choices = [...(shown()?.querySelectorAll<HTMLButtonElement>('.device-pairing-choice') ?? [])];
			for (const choice of choices) {
				click(choice);
			}
			// The command service runs a command once its activation settles.
			await timeout(0);
			assert.deepStrictEqual(popover.opened, [{ kind: 'create', anchor: choices[0] }, { kind: 'import', anchor: choices[1] }]);
		});
		test('returning to a page shows the same page rather than a new one', () => {
			const devicesPage = shown();
			click(button('connection'));
			const settingsPage = shown();
			assert.ok(devicesPage && settingsPage && settingsPage !== devicesPage, 'the devices and connection pages are not each shown');
			click(button('devices'));
			assert.strictEqual(shown(), devicesPage);
			click(button('connection'));
			assert.strictEqual(shown(), settingsPage);
		});
		test('a device name renders as text, never as markup', () => {
			devices.fireDevices([{ id: 'peer', name: '<b>Remote computer</b>', os: 'linux', presence: 'connected', hostname: null, direct: 0, relays: 0 }]);
			const page = shown();
			assert.ok(page);
			assert.ok(page.textContent.includes('<b>Remote computer</b>'));
			assert.strictEqual(page.querySelector('b'), null);
		});
		test('the devices page follows device changes until Home is disposed', () => {
			const page = shown();
			assert.ok(page);
			const back = button('devices');
			devices.fireDevices([{ id: 'peer', name: 'Remote computer', os: 'linux', presence: 'connected', hostname: null, direct: 0, relays: 0 }]);
			assert.ok(page.textContent.includes('Remote computer'), 'a device change was not shown');
			home.dispose();
			const disposed = page.textContent;
			click(back);
			devices.fireDevices([]);
			assert.strictEqual(page.textContent, disposed);
		});
	});
});
