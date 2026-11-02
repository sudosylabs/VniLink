// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import { $ } from '../../../../../base/browser/dom.js';
import { Event } from '../../../../../base/common/event.js';
import { Disposable, toDisposable } from '../../../../../base/common/lifecycle.js';
import { IMenuService } from '../../../../../platform/actions/common/actions.js';
import { MenuService } from '../../../../../platform/actions/common/menuService.js';
import { ICommandService } from '../../../../../platform/commands/common/commands.js';
import { IConfigurationService } from '../../../../../platform/configuration/common/configuration.js';
import { ContextKeyService } from '../../../../../platform/contextkey/browser/contextKeyService.js';
import { IContextKeyService } from '../../../../../platform/contextkey/common/contextkey.js';
import { IContextMenuService } from '../../../../../platform/contextview/browser/contextView.js';
import { SyncDescriptor } from '../../../../../platform/instantiation/common/descriptors.js';
import { ServiceCollection } from '../../../../../platform/instantiation/common/serviceCollection.js';
import { TestInstantiationService } from '../../../../../platform/instantiation/test/common/instantiationServiceMock.js';
import { ILogService, NullLogService } from '../../../../../platform/log/common/log.js';
import { INativeHostService } from '../../../../../platform/native/common/native.js';
import { CommandService } from '../../../../services/commands/commandService.js';
import { TestConfigurationService } from '../../../../services/configuration/test/common/testConfigurationService.js';
import { IDeviceService, type DeviceSummary } from '../../../../services/devices/deviceService.js';
import { TestDeviceService } from '../../../../services/devices/test/common/testDeviceService.js';
import { INativeRuntimeService } from '../../../../services/nativeRuntime/nativeRuntimeService.js';
import type { IDesktopSession } from '../../../../services/remoteDesktop/remoteDesktopService.js';
import { IRemoteDesktopService } from '../../../../services/remoteDesktop/remoteDesktopService.js';
import { ISystemActionService } from '../../../../services/systemActions/systemActionService.js';
import { ITabService } from '../../../../services/tabs/tabService.js';
import { TabService } from '../../../../services/tabs/tabServiceImpl.js';
import { IUIStateService } from '../../../../services/uiState/uiStateService.js';
import '../../remoteDesktop.contribution.js';
import { OpenRemoteDesktopAction, ViewScreenAction } from '../../remoteDesktopActions.js';
import { RemoteDesktopView } from '../../remoteDesktopView.js';
import { TestContextMenuService, TestDesktopHostService, TestDesktopSession, TestDesktopSystemActionService, TestRecordingRuntime, TestRemoteDesktopService, TestUIStateService } from './desktopViewFakes.js';

/** The computer the tabs show. */
export const studio: DeviceSummary = { id: 'studio', name: 'Studio PC', os: 'windows', presence: 'connected', hostname: 'studio-pc', direct: 1, relays: 0 };

/**
 * Remote desktop tabs built as the workbench builds them, over test services,
 * shown in a 1280 × 720 host below where the titlebar would be. Sessions are
 * made by `createSession`, test sessions by default.
 */
export class RemoteDesktopTabFixture<S extends IDesktopSession> extends Disposable {
	public readonly contextMenu = new TestContextMenuService();
	public readonly systemActions = this._register(new TestDesktopSystemActionService());
	public readonly uiState = new TestUIStateService();
	public readonly runtime = new TestRecordingRuntime();
	public readonly nativeHost = this._register(new TestDesktopHostService());
	public readonly configuration = this._register(new TestConfigurationService());
	public readonly devices = this._register(new TestDeviceService());
	public readonly remoteDesktop: TestRemoteDesktopService;
	public readonly tabService = this._register(new TabService());
	public readonly host = $('div');
	private readonly instantiationService: TestInstantiationService;
	private readonly created: S[] = [];

	constructor(createSession: (control: boolean) => S, device: DeviceSummary = studio, hostStyle = 'position:fixed;left:0;top:40px;width:1280px;height:720px;z-index:99;display:flex;') {
		super();
		this.remoteDesktop = new TestRemoteDesktopService(this.runtime, control => {
			const session = createSession(control);
			this.created.push(session);
			return session;
		});
		this._register(toDisposable(() => {
			this.remoteDesktop.dispose();
		}));
		this.devices.devices = [device];
		this.instantiationService = this._register(new TestInstantiationService(new ServiceCollection(
			[ICommandService, new SyncDescriptor(CommandService)],
			[IMenuService, new SyncDescriptor(MenuService)],
		), true, undefined, true));
		const services = this.instantiationService;
		services.stub(IContextMenuService, this.contextMenu);
		services.stub(ISystemActionService, this.systemActions);
		services.stub(IUIStateService, this.uiState);
		services.stub(INativeRuntimeService, this.runtime);
		services.stub(INativeHostService, this.nativeHost);
		services.stub(IConfigurationService, this.configuration);
		services.stub(IContextKeyService, this._register(new ContextKeyService(this.configuration)));
		services.stub(ILogService, new NullLogService());
		services.stub(ITabService, this.tabService);
		services.stub(IDeviceService, this.devices);
		services.stub(IRemoteDesktopService, this.remoteDesktop);
		this.host.style.cssText = hostStyle;
		document.body.append(this.host);
		this._register(toDisposable(() => {
			this.host.remove();
		}));
	}

	/** The sessions opened so far, in order. */
	public get sessions(): readonly S[] {
		return this.created;
	}

	/** Opens the tab Remote Desktop (`control`) or View Screen opens for `deviceId`, and shows it in the host. */
	public open(control = true, deviceId = this.devices.devices[0].id): RemoteDesktopView {
		this.instantiationService.invokeFunction(accessor => {
			if (control) {
				new OpenRemoteDesktopAction().run(accessor, deviceId);
			} else {
				new ViewScreenAction().run(accessor, deviceId);
			}
		});
		const view = this.tabService.activeTab;
		if (!(view instanceof RemoteDesktopView)) {
			throw new Error('The action opened no remote desktop tab');
		}
		this.host.replaceChildren(view.element);
		return view;
	}

	/** Resolves once `count` sessions have been opened. */
	public async opened(count: number): Promise<void> {
		while (this.sessions.length < count) {
			await Event.toPromise(this.remoteDesktop.onDidOpen);
		}
	}
}

/** Clicks `element`, as a pointer does; nothing when there is none. */
export function click(element: Element | null | undefined): void {
	element?.dispatchEvent(new MouseEvent('click', { bubbles: true, cancelable: true }));
}

/**
 * Resolves once `condition` holds, checking it on each change under `root`
 * and each painted frame: for what the page shows after its own timers,
 * decoders and frames.
 */
export function rendered(root: Node, condition: () => boolean): Promise<void> {
	return new Promise(resolve => {
		let frame = 0;
		const check = (): boolean => {
			if (!condition()) {
				return false;
			}
			observer.disconnect();
			cancelAnimationFrame(frame);
			resolve();
			return true;
		};
		const observer = new MutationObserver(() => {
			check();
		});
		const tick = (): void => {
			if (!check()) {
				frame = requestAnimationFrame(tick);
			}
		};
		observer.observe(root, { subtree: true, childList: true, attributes: true, characterData: true });
		tick();
	});
}

/** Tabs over {@link TestDesktopSession}s, which the test plays. */
export function createTabFixture(device: DeviceSummary = studio, hostStyle?: string): RemoteDesktopTabFixture<TestDesktopSession> {
	return new RemoteDesktopTabFixture(control => new TestDesktopSession(control), device, hostStyle);
}

/** One open tab and what a test reads and does in it. */
export class DesktopTab {
	constructor(public readonly view: RemoteDesktopView, private readonly contextMenu: TestContextMenuService) { }

	public get element(): HTMLElement {
		return this.view.element;
	}

	public get overlay(): HTMLElement {
		return this.required('.desktop-overlay');
	}

	public get canvas(): HTMLCanvasElement {
		const canvas = this.required('.desktop-screen');
		if (!(canvas instanceof HTMLCanvasElement)) {
			throw new Error('The screen is not a canvas');
		}
		return canvas;
	}

	public get stage(): HTMLElement {
		return this.required('.desktop-stage');
	}

	public get mark(): HTMLElement {
		return this.required('.desktop-pointer');
	}

	public required(selector: string): HTMLElement {
		const element = this.element.querySelector<HTMLElement>(selector);
		if (!element) {
			throw new Error(`The tab has no ${selector}`);
		}
		return element;
	}

	/** Resolves once `condition` holds in this tab. */
	public when(condition: () => boolean): Promise<void> {
		return rendered(this.element, condition);
	}

	public text(selector: string): string {
		return this.element.querySelector(selector)?.textContent ?? '';
	}

	/** What the stats line says the tab is doing: Controlling, Viewing, Not connected. */
	public role(): string {
		return this.text('.desktop-stats-role');
	}

	/** A control of the session bar, by what it says to assistive technology. */
	public control(label: string): HTMLButtonElement {
		const found = [...this.element.querySelectorAll<HTMLButtonElement>('.desktop-bar button')].find(button => button.getAttribute('aria-label')?.startsWith(label));
		if (!found) {
			throw new Error(`The bar has no ${label} control`);
		}
		return found;
	}

	/** The labels of the bar's shown controls. */
	public controls(): string[] {
		return [...this.element.querySelectorAll<HTMLButtonElement>('.desktop-bar button')].filter(button => !button.hidden).map(button => button.getAttribute('aria-label') ?? '');
	}

	/** The detail named `term` in the expanded picture details. */
	public detail(term: string): string {
		const terms = [...this.element.querySelectorAll('.desktop-stats-details dt')];
		return terms.find(node => node.textContent === term)?.nextElementSibling?.textContent ?? '';
	}

	/** Opens the bar menu of `menu` and runs its item labelled `label`. */
	public pick(menu: HTMLButtonElement, label: string): void {
		click(menu);
		const action = this.contextMenu.menus.at(-1)?.actions.find(item => item.label === label);
		if (!action) {
			throw new Error(`The menu has no ${label}: ${this.contextMenu.lastMenu().join()}`);
		}
		void action.run();
	}

	/** The colour drawn at `x`, `y` of the picture, as `r,g,b`. */
	public shownColour(x = 960, y = 540): string {
		const [red, green, blue] = this.canvas.getContext('2d')?.getImageData(x, y, 1, 1).data ?? [];
		return `${String(red)},${String(green)},${String(blue)}`;
	}

	public press(target: HTMLElement, type: 'keydown' | 'keyup', code: string, key: string, init: KeyboardEventInit = {}): void {
		target.dispatchEvent(new KeyboardEvent(type, { code, key, bubbles: true, cancelable: true, ...init }));
	}
}
