// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'assert';
import '../../../../media/workbench.css';
import '../../media/titlebarpart.css';
import '../../media/modernTabs.css';
import { $ } from '../../../../../base/browser/dom.js';
import { timeout } from '../../../../../base/common/async.js';
import { Event } from '../../../../../base/common/event.js';
import { Icons } from '../../../../../base/common/icons.js';
import { Disposable } from '../../../../../base/common/lifecycle.js';
import { ThemeIcon } from '../../../../../base/common/themables.js';
import { ensureNoDisposablesAreLeakedInTestSuite } from '../../../../../base/test/common/utils.js';
import { runWithFakedTimers } from '../../../../../base/test/common/virtualScheduling/index.js';
import { Action2, IMenuService, registerAction2 } from '../../../../../platform/actions/common/actions.js';
import { MenuService } from '../../../../../platform/actions/common/menuService.js';
import { ICommandService } from '../../../../../platform/commands/common/commands.js';
import { IConfigurationService } from '../../../../../platform/configuration/common/configuration.js';
import { ContextKeyService } from '../../../../../platform/contextkey/browser/contextKeyService.js';
import { IContextKeyService, RawContextKey, type IContextKey } from '../../../../../platform/contextkey/common/contextkey.js';
import { IEngineService } from '../../../../../platform/engine/common/engineService.js';
import { TestEngineService } from '../../../../../platform/engine/test/common/testEngineService.js';
import { TestInstantiationService } from '../../../../../platform/instantiation/test/common/instantiationServiceMock.js';
import { ILogService, NullLogService } from '../../../../../platform/log/common/log.js';
import { IThemeService } from '../../../../../platform/theme/common/themeService.js';
import { ActiveTabContext } from '../../../../contextkeys.js';
import { WorkbenchContextKeysHandler } from '../../../../contextkeysHandler.js';
import { CommandService } from '../../../../services/commands/commandService.js';
import { ITabService, type WorkbenchTab } from '../../../../services/tabs/tabService.js';
import { TabService } from '../../../../services/tabs/tabServiceImpl.js';
import { TestConfigurationService } from '../../../../services/configuration/test/common/testConfigurationService.js';
import { ViewsPart } from '../../../views/viewsPart.js';
import { TitlebarTabsControl } from '../../titlebarTabsControl.js';
import { ViewActionsMenu } from '../../viewActionsToolbar.js';

/** Presses `element` as a pointer does. */
function click(element: Element | null | undefined): void {
	element?.dispatchEvent(new MouseEvent('click', { bubbles: true, cancelable: true }));
}

/** Whether the test's slow action may start; a precondition, as real actions use. */
const BusyAllowedContext = new RawContextKey<boolean>('test.busyAllowed', true);

/** The menu service waits 50 ms to rebuild a menu and 50 more to announce it. */
const menuDebounce = 100;

/**
 * The titlebar's tabs and the views part over a real tab, menu, command and
 * context key service, with a contextual action for the tabs `first` and
 * `second` and a slow one, `busy`, for the tab `dynamic`.
 */
class TabsFixture extends Disposable {
	public readonly titlebar = $('div');
	public readonly root = $('div');
	/** The page under the tabs. */
	public readonly content = $('div');
	public readonly tabService = this._register(new TabService());
	/** The actions that ran, by tab. */
	public readonly ran: string[] = [];
	public readonly busyAllowed: IContextKey<boolean>;
	public readonly control: TitlebarTabsControl;
	/** Views disposed so far. */
	public disposed = 0;
	private finishBusy: (() => void) | undefined;

	constructor() {
		super();
		this.titlebar.className = 'part titlebar';
		this.titlebar.style.cssText = 'position:fixed;left:0;top:80px;width:440px;height:35px;z-index:99';
		this.root.className = 'titlebar-container';
		this.root.style.padding = '0';
		this.titlebar.append(this.root);
		document.body.append(this.titlebar);
		const content = this.content;
		content.style.backgroundColor = 'var(--canvas)';
		this.titlebar.append(content);
		const instantiationService = this._register(new TestInstantiationService());
		const configuration = this._register(new TestConfigurationService());
		instantiationService.stub(IConfigurationService, configuration);
		instantiationService.stub(ITabService, this.tabService);
		instantiationService.stub(ILogService, new NullLogService());
		instantiationService.stub(IThemeService, { onDidColorThemeChange: Event.None, getColorTheme: () => ({ background: '#000000', foreground: '#ffffff', inactiveForeground: '#888888' }) });
		instantiationService.stub(IEngineService, this._register(new TestEngineService(() => Promise.reject(new Error('Unused')))));
		const contextKeys = this._register(new ContextKeyService(configuration));
		instantiationService.stub(IContextKeyService, contextKeys);
		instantiationService.stub(ICommandService, this._register(instantiationService.createInstance(CommandService)));
		instantiationService.stub(IMenuService, this._register(instantiationService.createInstance(MenuService)));
		this._register(instantiationService.createInstance(WorkbenchContextKeysHandler));
		this.busyAllowed = BusyAllowedContext.bindTo(contextKeys);
		const ran = this.ran;
		const viewAction = (id: string) => class extends Action2 {
			constructor() {
				super({ id: `test.action.${id}`, title: `Action ${id}`, icon: ThemeIcon.fromId(Icons.refresh), menu: { id: ViewActionsMenu, when: ActiveTabContext.isEqualTo(id) } });
			}

			public override run(): void {
				ran.push(id);
			}
		};
		this._register(registerAction2(viewAction('first')));
		this._register(registerAction2(viewAction('second')));
		const busyStarted = (finish: () => void): void => {
			this.finishBusy = finish;
		};
		this._register(registerAction2(class extends Action2 {
			constructor() {
				super({ id: 'test.action.busy', title: 'Busy', precondition: BusyAllowedContext, menu: { id: ViewActionsMenu, when: ActiveTabContext.isEqualTo('dynamic') } });
			}

			public override run(): Promise<void> {
				return new Promise<void>(busyStarted).then(() => {
					ran.push('busy');
				});
			}
		}));
		this.control = this._register(instantiationService.createInstance(TitlebarTabsControl, this.root));
		const views = this._register(instantiationService.createInstance(ViewsPart));
		views.create(content);
	}

	/** A view named `id` that counts its disposal. */
	public view(id: string): WorkbenchTab {
		const element = $('main');
		element.textContent = id;
		return {
			id,
			label: `View ${id}`,
			element,
			dispose: () => {
				this.disposed++;
				element.remove();
			},
		};
	}

	/** Opens Home and the views `first` and `second`; `second` is active. */
	public openThree(): { readonly home: WorkbenchTab; readonly first: WorkbenchTab; readonly second: WorkbenchTab } {
		const home = { ...this.view('home'), permanent: true };
		const first = this.view('first');
		const second = this.view('second');
		this.tabService.open(home);
		this.tabService.open(first);
		this.tabService.open(second);
		return { home, first, second };
	}

	/** Lets the menu service rebuild and announce the toolbar; time is virtual where this is called. */
	public settle(): Promise<void> {
		return timeout(menuDebounce);
	}

	public action(): HTMLButtonElement | null {
		return this.root.querySelector<HTMLButtonElement>('.view-action');
	}

	public finish(): void {
		this.finishBusy?.();
	}

	public override dispose(): void {
		this.finish();
		// Views still open are the test's to release, as the workbench releases its own.
		for (const tab of [...this.tabService.tabs]) {
			tab.dispose();
		}
		super.dispose();
		this.titlebar.remove();
	}
}

suite('TitlebarTabsControl', () => {
	const disposables = ensureNoDisposablesAreLeakedInTestSuite();
	let fixture: TabsFixture;
	setup(() => {
		fixture = disposables.add(new TabsFixture());
	});
	test('Home has no close button and cannot be closed or dragged', async () => {
		const home = { ...fixture.view('home'), permanent: true };
		fixture.tabService.open(home);
		assert.strictEqual(fixture.root.querySelector('[data-tab-id="home"] .tab-close'), null);
		const homeTab = fixture.root.querySelector<HTMLElement>('[data-tab-id="home"]');
		homeTab?.dispatchEvent(new KeyboardEvent('keydown', { key: 'Delete', bubbles: true }));
		homeTab?.dispatchEvent(new MouseEvent('auxclick', { button: 1, bubbles: true }));
		await Promise.resolve();
		assert.strictEqual(fixture.tabService.tabs[0], home);
		assert.strictEqual(homeTab?.draggable, false);
	});
	test('only the active view is shown, and switching keeps each view', () => {
		const { first, second } = fixture.openThree();
		assert.deepStrictEqual([first.element.hidden, second.element.hidden], [true, false]);
		fixture.tabService.select(first.id);
		assert.deepStrictEqual([first.element.hidden, second.element.hidden], [false, true]);
		assert.strictEqual(fixture.disposed, 0, 'switching disposed a view');
	});
	test('the active tab is drawn as a rounded, outlined fill in the page\'s colour, joined to the page', () => {
		fixture.openThree();
		assert.strictEqual(getComputedStyle(fixture.titlebar).boxShadow, 'none', 'the titlebar divider crosses the active tab');
		const fill = fixture.root.querySelector<HTMLElement>('[aria-selected="true"] .tab-fill');
		assert.ok(fill);
		const style = getComputedStyle(fill);
		assert.ok(Number.parseFloat(style.borderTopLeftRadius) > 0, 'the active tab is not rounded');
		assert.strictEqual(style.borderBottomWidth, '0px', 'the joined active tab has a bottom divider');
		assert.strictEqual(Number.parseFloat(getComputedStyle(fill, '::before').bottom), -Number.parseFloat(style.bottom), 'the side strokes overlap the reverse shoulders');
		const page = getComputedStyle(fixture.content).backgroundColor;
		assert.notStrictEqual(page, 'rgba(0, 0, 0, 0)', 'the page has no colour to compare with');
		assert.strictEqual(style.backgroundColor, page, 'the active tab is not the page\'s colour');
		assert.ok(style.boxShadow.startsWith(`${page} `), 'the active tab stops short of the page');
		assert.strictEqual(style.backgroundClip, 'padding-box', 'the outline lies over the tab\'s fill, not the titlebar the divider lies over');
		// A 1px border is snapped to whole device pixels: one device pixel, 0.8px, at Windows' 125% scale.
		assert.strictEqual(Math.round(Number.parseFloat(style.borderTopWidth) * window.devicePixelRatio), Math.max(1, Math.floor(window.devicePixelRatio)));
		assert.notStrictEqual(style.borderTopColor, 'rgba(0, 0, 0, 0)', 'the active tab lost its selection outline');
	});
	test('a view drawn on Mica has a tab of Mica; on glass and vibrancy its tab is the page\'s', () => {
		const root = document.documentElement;
		try {
			fixture.tabService.open({ ...fixture.view('welcome'), surface: 'material' });
			const fill = fixture.root.querySelector<HTMLElement>('[aria-selected="true"] .tab-fill');
			assert.ok(fill);
			const page = getComputedStyle(fixture.content).backgroundColor;
			const backgrounds = Object.fromEntries((['mica', 'glass', 'vibrancy', 'none'] as const).map(material => {
				root.dataset.windowMaterial = material;
				return [material, getComputedStyle(fill).backgroundColor];
			}));
			assert.deepStrictEqual(backgrounds, { mica: 'rgba(0, 0, 0, 0)', glass: page, vibrancy: page, none: page });
		} finally {
			delete root.dataset.windowMaterial;
		}
	});
	test('each shoulder of the active tab is one shape, its line and fill drawn around the same centre', () => {
		fixture.openThree();
		const fill = fixture.root.querySelector<HTMLElement>('[aria-selected="true"] .tab-fill');
		assert.ok(fill);
		for (const [side, centre] of [['::before', 'at 0px 0px'], ['::after', 'at 100% 0px']] as const) {
			const shoulder = getComputedStyle(fill, side);
			assert.match(shoulder.backgroundImage, new RegExp(`^radial-gradient\\(circle ${centre}, rgba\\(0, 0, 0, 0\\)`), `the ${side} shoulder is not one gradient`);
			assert.strictEqual(shoulder.borderBottomWidth, '0px', `the ${side} shoulder still draws its line as a border`);
			assert.strictEqual(shoulder.boxShadow, 'none', `the ${side} shoulder still fills with a shadow`);
		}
	});
	test('each shoulder\'s line continues the side line and ends where the divider resumes', () => {
		const { first } = fixture.openThree();
		fixture.tabService.select(first.id);
		const tab = fixture.root.querySelector<HTMLElement>('[aria-selected="true"]');
		const fill = tab?.querySelector<HTMLElement>('.tab-fill');
		assert.ok(tab && fill && tab.previousElementSibling && tab.nextElementSibling);
		const px = (value: string) => Number.parseFloat(value);
		const bounds = fill.getBoundingClientRect();
		// Not `clientLeft`, which rounds: the 1px side line is 0.8px at Windows' 125% scale.
		const side = px(getComputedStyle(fill).borderLeftWidth);
		const { previousElementSibling: previous, nextElementSibling: next } = tab;
		const shoulders = [
			// Positioned from the fill's padding box; each circle is centred on the shape's outer top corner.
			{
				pseudo: '::before',
				sideLine: bounds.left + side / 2,
				centre: (style: CSSStyleDeclaration) => bounds.right - side - px(style.right) - px(style.width),
				divider: previous.getBoundingClientRect().right - px(getComputedStyle(previous, '::after').right),
			},
			{
				pseudo: '::after',
				sideLine: bounds.right - side / 2,
				centre: (style: CSSStyleDeclaration) => bounds.left + side + px(style.left) + px(style.width),
				divider: next.getBoundingClientRect().left + px(getComputedStyle(next, '::after').left),
			},
		];
		for (const { pseudo, sideLine, centre, divider } of shoulders) {
			const style = getComputedStyle(fill, pseudo);
			const stops = [...style.backgroundImage.matchAll(/\) ([\d.]+)px/g)].map(match => Number(match[1]));
			assert.ok(stops.length >= 3, `the ${pseudo} shoulder has no line`);
			// The line is the stops from transparent to the line's colour and back out.
			const radius = (stops[1] + stops[2]) / 2;
			const x = centre(style);
			assert.ok(Math.abs(Math.abs(x - sideLine) - radius) < 0.25, `the ${pseudo} shoulder's line starts ${(Math.abs(x - sideLine) - radius).toFixed(2)}px off the side line`);
			assert.ok(Math.abs(divider - x) <= side, `the divider resumes ${(divider - x).toFixed(2)}px from where the ${pseudo} shoulder ends`);
		}
	});
	test('selecting a tab does not change its label colour', () => {
		fixture.openThree();
		const active = fixture.root.querySelector<HTMLElement>('[aria-selected="true"]');
		const inactive = fixture.root.querySelector<HTMLElement>('[aria-selected="false"]');
		assert.ok(active && inactive);
		assert.strictEqual(getComputedStyle(active).color, getComputedStyle(inactive).color);
	});
	test('the toolbar shows the active tab\'s actions, pressed rather than dragged', () => runWithFakedTimers({}, async () => {
		const { first } = fixture.openThree();
		fixture.tabService.select(first.id);
		await fixture.settle();
		const action = fixture.action();
		assert.strictEqual(action?.getAttribute('aria-label'), 'Action first');
		// The titlebar drags the window; its actions are pressed, not dragged.
		assert.strictEqual(getComputedStyle(action).getPropertyValue('-webkit-app-region'), 'no-drag');
	}));
	test('an action runs against the active tab', () => runWithFakedTimers({}, async () => {
		const { first } = fixture.openThree();
		fixture.tabService.select(first.id);
		await fixture.settle();
		click(fixture.action());
		await fixture.settle();
		assert.deepStrictEqual(fixture.ran, ['first']);
	}));
	test('a precondition disables and enables its action', () => runWithFakedTimers({}, async () => {
		fixture.tabService.open(fixture.view('dynamic'));
		await fixture.settle();
		fixture.busyAllowed.set(false);
		await fixture.settle();
		assert.strictEqual(fixture.action()?.disabled, true);
		fixture.busyAllowed.set(true);
		await fixture.settle();
		assert.strictEqual(fixture.action()?.disabled, false);
	}));
	test('a running action is disabled, runs once, and finishing it leaves the active tab alone', () => runWithFakedTimers({}, async () => {
		const { second } = fixture.openThree();
		fixture.tabService.open(fixture.view('dynamic'));
		await fixture.settle();
		const busy = fixture.action();
		click(busy);
		assert.strictEqual(busy?.disabled, true, 'the running action is not disabled');
		click(busy);
		fixture.tabService.select(second.id);
		await fixture.settle();
		assert.strictEqual(fixture.action()?.getAttribute('aria-label'), 'Action second', 'the toolbar kept the previous tab\'s actions');
		fixture.finish();
		await fixture.settle();
		assert.deepStrictEqual(fixture.ran, ['busy']);
		assert.strictEqual(fixture.tabService.activeTab, second);
	}));
	suite('many tabs', () => {
		let scrollable: HTMLElement;
		let toolbar: HTMLElement;
		setup(() => {
			fixture.openThree();
			for (let index = 0; index < 12; index++) {
				fixture.tabService.open(fixture.view(String(index)));
			}
			const strip = fixture.root.querySelector<HTMLElement>('.tabs-scrollable');
			const actions = fixture.root.querySelector<HTMLElement>('.view-actions');
			assert.ok(strip && actions);
			scrollable = strip;
			toolbar = actions;
		});
		test('the tabs scroll sideways without covering the contextual actions', () => {
			assert.ok(scrollable.scrollWidth > scrollable.clientWidth, 'the tabs do not overflow');
			assert.ok(scrollable.getBoundingClientRect().right <= toolbar.getBoundingClientRect().left + 1);
		});
		test('the active tab is revealed', () => {
			assert.ok(scrollable.scrollLeft > 0);
		});
		test('a change to other tabs keeps the strip where it was scrolled', () => {
			scrollable.scrollLeft = 0;
			fixture.tabService.move('5', 4);
			assert.strictEqual(scrollable.scrollLeft, 0);
		});
		test('ArrowRight selects the next tab', () => {
			fixture.tabService.select('0');
			fixture.root.querySelector<HTMLElement>('[aria-selected="true"]')?.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowRight', bubbles: true }));
			assert.strictEqual(fixture.tabService.activeTab?.id, '1');
		});
	});
});
