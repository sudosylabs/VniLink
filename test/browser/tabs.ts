// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import { Emitter, Event } from '../../src/base/common/event.js';
import { Icons } from '../../src/base/common/icons.js';
import { DisposableStore } from '../../src/base/common/lifecycle.js';
import { ThemeIcon } from '../../src/base/common/themables.js';
import { Action2, IMenuService, registerAction2 } from '../../src/platform/actions/common/actions.js';
import { MenuService } from '../../src/platform/actions/common/menuService.js';
import { ICommandService } from '../../src/platform/commands/common/commands.js';
import { IConfigurationService } from '../../src/platform/configuration/common/configuration.js';
import { ContextKeyService } from '../../src/platform/contextkey/browser/contextKeyService.js';
import { ContextKeyExpr, IContextKeyService, RawContextKey } from '../../src/platform/contextkey/common/contextkey.js';
import { IEngineService } from '../../src/platform/engine/common/engineService.js';
import { SyncDescriptor } from '../../src/platform/instantiation/common/descriptors.js';
import { InstantiationService } from '../../src/platform/instantiation/common/instantiationService.js';
import { ServiceCollection } from '../../src/platform/instantiation/common/serviceCollection.js';
import { ILogService, NullLogService } from '../../src/platform/log/common/log.js';
import { IThemeService } from '../../src/platform/theme/common/themeService.js';
import { ActiveTabContext } from '../../src/workbench/contextkeys.js';
import { WorkbenchContextKeysHandler } from '../../src/workbench/contextkeysHandler.js';
import { TitlebarTabsControl } from '../../src/workbench/parts/titlebar/titlebarTabsControl.js';
import { ViewActionsMenu } from '../../src/workbench/parts/titlebar/viewActionsToolbar.js';
import { ViewsPart } from '../../src/workbench/parts/views/viewsPart.js';
import { CommandService } from '../../src/workbench/services/commands/commandService.js';
import { ITabService, type WorkbenchTab } from '../../src/workbench/services/tabs/tabService.js';
import { TabService } from '../../src/workbench/services/tabs/tabServiceImpl.js';

function assert(condition: boolean, message: string): void {
	if (!condition) {
		throw new Error(message);
	}
}

/** Whether the test's slow action may start; a precondition, as real actions use. */
const BusyAllowedContext = new RawContextKey<boolean>('test.busyAllowed', true);

export async function verifyTabs(): Promise<boolean> {
	const disposables = new DisposableStore();
	const fixture = document.createElement('div');
	fixture.className = 'part titlebar';
	fixture.style.cssText = 'position:fixed;left:0;top:80px;width:440px;height:35px;z-index:99';
	const root = document.createElement('div');
	root.className = 'titlebar-container';
	root.style.padding = '0';
	fixture.append(root);
	document.body.append(fixture);
	const content = document.createElement('div');
	content.style.backgroundColor = 'var(--canvas)';
	fixture.append(content);
	const tabService = disposables.add(new TabService());
	const services = disposables.add(new InstantiationService(new ServiceCollection(
		[ITabService, tabService],
		[ILogService, new NullLogService()],
		[IConfigurationService, { _serviceBrand: undefined, onDidChangeConfiguration: Event.None, getValue: () => undefined }],
		[IThemeService, { _serviceBrand: undefined, onDidColorThemeChange: Event.None, getColorTheme: () => ({ background: '#000000', foreground: '#ffffff', inactiveForeground: '#888888' }) }],
		[IEngineService, { _serviceBrand: undefined, available: true, restartRequired: false, onDidChangeAvailability: Event.None, onDidChangeRestartRequired: Event.None, onDidReceiveEvents: Event.None, execute: () => Promise.reject(new Error('Unused')), openDataChannel: () => Promise.reject(new Error('Unused')) }],
		[IContextKeyService, new SyncDescriptor(ContextKeyService)],
		[ICommandService, new SyncDescriptor(CommandService)],
		[IMenuService, new SyncDescriptor(MenuService)],
	), true));
	disposables.add(services.createInstance(WorkbenchContextKeysHandler));
	const busyAllowed = BusyAllowedContext.bindTo(services.invokeFunction(accessor => accessor.get(IContextKeyService)));
	const ran: string[] = [];
	let finish: (() => void) | undefined;
	const viewAction = (id: string) => class extends Action2 {
		constructor() {
			super({ id: `test.action.${id}`, title: `Action ${id}`, icon: ThemeIcon.fromId(Icons.refresh), menu: { id: ViewActionsMenu, when: ActiveTabContext.isEqualTo(id) } });
		}

		public override run(): void {
			ran.push(id);
		}
	};
	disposables.add(registerAction2(viewAction('first')));
	disposables.add(registerAction2(viewAction('second')));
	disposables.add(registerAction2(class extends Action2 {
		constructor() {
			super({ id: 'test.action.busy', title: 'Busy', precondition: BusyAllowedContext, menu: { id: ViewActionsMenu, when: ContextKeyExpr.equals(ActiveTabContext.key, 'dynamic') } });
		}

		public override run(): Promise<void> {
			return new Promise<void>(resolve => {
				finish = resolve;
			}).then(() => {
				ran.push('busy');
			});
		}
	}));
	const tabs = disposables.add(services.createInstance(TitlebarTabsControl, root));
	const views = disposables.add(services.createInstance(ViewsPart));
	views.create(content);
	let disposed = 0;
	const changed = disposables.add(new Emitter<void>());
	function view(id: string): WorkbenchTab {
		const element = document.createElement('main');
		element.textContent = id;
		return {
			id,
			label: `View ${id}`,
			element,
			dispose: () => {
				disposed++;
				element.remove();
			},
		};
	}
	const settle = async (): Promise<void> => {
		for (let step = 0; step < 5; step++) {
			await new Promise(resolve => setTimeout(resolve, 60));
		}
	};
	try {
		const home = { ...view('home'), permanent: true };
		tabService.open(home);
		assert(root.querySelector('[data-tab-id="home"] .tab-close') === null, 'Home has a close button');
		const homeTab = root.querySelector<HTMLElement>('[data-tab-id="home"]');
		homeTab?.dispatchEvent(new KeyboardEvent('keydown', { key: 'Delete', bubbles: true }));
		homeTab?.dispatchEvent(new MouseEvent('auxclick', { button: 1, bubbles: true }));
		await Promise.resolve();
		assert(tabService.tabs[0] === home && homeTab?.draggable === false, 'Home can be closed or dragged');
		const first = view('first');
		const second = view('second');
		tabService.open(first);
		tabService.open(second);
		assert(first.element.hidden === true && second.element.hidden === false, 'Inactive view is not hidden');
		tabService.select(first.id);
		assert(first.element.hidden === false && second.element.hidden === true, 'View state was not preserved on switching');
		assert(disposed === 0, 'Switching disposed a view');
		assert(getComputedStyle(fixture).boxShadow === 'none', 'Titlebar divider still crosses the transparent active tab');
		const fill = root.querySelector<HTMLElement>('[aria-selected="true"] .tab-fill');
		assert(fill !== null && getComputedStyle(fill).borderTopLeftRadius === '12px', 'Modern tab fill is not rounded');
		assert(fill !== null && getComputedStyle(fill).borderBottomWidth === '0px', 'Connected active tab has a bottom divider');
		assert(fill !== null && getComputedStyle(fill).bottom === '7px' && getComputedStyle(fill, '::before').bottom === '-7px', 'Active side strokes overlap the reverse shoulders');
		const close = root.querySelector<HTMLElement>('[aria-selected="true"] .tab-close');
		assert(close !== null && getComputedStyle(close).borderRadius === '3px', 'Close hover target is not square');
		assert(fill !== null && getComputedStyle(fill).backgroundColor === 'rgba(0, 0, 0, 0)', 'Active tab masked the titlebar background');
		// A 1px border is snapped to whole device pixels: one device pixel,
		// 0.8px, at Windows' 125% scale.
		assert(fill !== null && Math.round(Number.parseFloat(getComputedStyle(fill).borderTopWidth) * window.devicePixelRatio) === Math.max(1, Math.floor(window.devicePixelRatio)) && getComputedStyle(fill).borderTopColor !== 'rgba(0, 0, 0, 0)', 'Active tab lost its selection outline');
		const active = root.querySelector<HTMLElement>('[aria-selected="true"]');
		const inactive = root.querySelector<HTMLElement>('[aria-selected="false"]');
		assert(active !== null && inactive !== null && getComputedStyle(active).color === getComputedStyle(inactive).color, 'Tab selection still changed the label color');
		await settle();
		const action = root.querySelector<HTMLButtonElement>('.view-action');
		assert(action?.getAttribute('aria-label') === 'Action first', 'Toolbar does not follow the active tab');
		// The titlebar drags the window; its actions are pressed, not dragged.
		assert(action !== null && getComputedStyle(action).getPropertyValue('-webkit-app-region') === 'no-drag', 'A titlebar action drags the window');
		action?.click();
		await settle();
		assert(ran.join() === 'first', 'Action ran against the wrong view');
		const dynamic = view('dynamic');
		tabService.open(dynamic);
		await settle();
		busyAllowed.set(false);
		await settle();
		assert(root.querySelector<HTMLButtonElement>('.view-action')?.disabled === true, 'Precondition did not disable the action');
		busyAllowed.set(true);
		await settle();
		const busy = root.querySelector<HTMLButtonElement>('.view-action');
		assert(busy?.disabled === false, 'Precondition did not enable the action');
		busy?.click();
		assert(busy?.disabled === true, 'Running action is not disabled');
		busy?.click();
		tabService.select(second.id);
		await settle();
		assert(root.querySelector<HTMLButtonElement>('.view-action')?.getAttribute('aria-label') === 'Action second', 'Toolbar kept the previous tab\'s actions');
		finish?.();
		await settle();
		assert(ran.join() === 'first,busy', 'A running action started twice or never finished');
		assert(tabService.activeTab === second, 'A finishing action changed the active tab');
		const guarded = { ...view('guarded'), canClose: () => false };
		tabService.open(guarded);
		assert(!(await tabService.close(guarded.id)), 'Close veto ignored');
		for (let index = 0; index < 12; index++) {
			tabService.open(view(String(index)));
		}
		const scrollable = root.querySelector<HTMLElement>('.tabs-scrollable');
		const toolbar = root.querySelector<HTMLElement>('.view-actions');
		assert(scrollable !== null && toolbar !== null && scrollable.scrollWidth > scrollable.clientWidth, 'Tabs do not overflow horizontally');
		if (scrollable && toolbar) {
			assert(scrollable.getBoundingClientRect().right <= toolbar.getBoundingClientRect().left + 1, 'Tabs overlap the contextual actions');
			assert(scrollable.scrollLeft > 0, 'Active tab not revealed');
			scrollable.scrollLeft = 0;
			changed.fire();
			assert(scrollable.scrollLeft === 0, 'An update moved the manually scrolled tab strip');
		}
		tabService.select('0');
		root.querySelector<HTMLElement>('[aria-selected="true"]')?.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowRight', bubbles: true }));
		assert(tabService.activeTab?.id === '1', 'Arrow navigation failed');
		tabService.move('1', 0);
		assert(tabService.tabs[0] === home && tabService.tabs[1]?.id === '1', 'Reorder displaced Home');
		await tabService.close('1');
		assert(tabService.activeTab?.id !== '1', 'Closed tab is still active');
		assert(tabs instanceof TitlebarTabsControl, 'Tabs control was not created');
		return true;
	} finally {
		finish?.();
		disposables.dispose();
		fixture.remove();
	}
}
