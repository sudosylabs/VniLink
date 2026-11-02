/*---------------------------------------------------------------------------------------------
 *  Copyright (c) Microsoft Corporation. All rights reserved.
 *  Licensed under the MIT License. See LICENSE.vscode.txt in the project root for license information.
 *  Adapted from VS Code 1.141.0 src/vs/workbench/services/driver/browser/driver.ts.
 *--------------------------------------------------------------------------------------------*/
// ---------------------------------------------------------------------------------------------
// Modifications Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Modifications licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only AND MIT
// ---------------------------------------------------------------------------------------------

import { getTopLeftOffset, isHTMLElement } from '../../../base/browser/dom.js';
import { mainWindow } from '../../../base/browser/window.js';
import { coalesce } from '../../../base/common/arrays.js';
import { DeferredPromise } from '../../../base/common/async.js';
import { DisposableStore } from '../../../base/common/lifecycle.js';
import { context, ipcRenderer } from '../../../base/parts/sandbox/electron-browser/globals.js';
import { ICommandService } from '../../../platform/commands/common/commands.js';
import { IDataChannelService } from '../../../platform/engine/common/dataChannelService.js';
import type { EngineCommand, EngineReply } from '../../../platform/engine/common/engine.js';
import { IEngineService } from '../../../platform/engine/common/engineService.js';
import type { IInstantiationService } from '../../../platform/instantiation/common/instantiation.js';
import { IMainProcessService } from '../../../platform/ipc/common/mainProcessService.js';
import { INativeHostService } from '../../../platform/native/common/native.js';
import { IWorkbenchConfigurationService } from '../configuration/configurationService.js';
import { IPermissionService } from '../permissions/permissionService.js';
import type { IElement, IElementLayout, IEngineRestartStory, IRendererExposure, IWindowDriver } from './windowDriver.js';

/** `promise`, or a failure saying `message` once `ms` have passed without it. */
async function within<T>(promise: Promise<T>, ms: number, message: () => string): Promise<T> {
	let timer: ReturnType<typeof setTimeout> | undefined;
	const late = new Promise<never>((_resolve, reject) => {
		timer = setTimeout(() => {
			reject(new Error(message()));
		}, ms);
	});
	try {
		return await Promise.race([promise, late]);
	} finally {
		clearTimeout(timer);
	}
}

/** Runs `body` and answers through a promise, as every driver call across the process does. */
function answer<T>(body: () => T): Promise<T> {
	try {
		return Promise.resolve(body());
	} catch (error) {
		return Promise.reject(error instanceof Error ? error : new Error(String(error)));
	}
}

/**
 * Automation access to the window: its elements, as VS Code's driver gives,
 * and the real services the smoke suite checks. Only registered for
 * isolated smoke-test launches.
 */
export class BrowserWindowDriver implements IWindowDriver {

	private readonly restart = new DisposableStore();
	private restartStory: { story: string[]; back: DeferredPromise<void> } | undefined;
	private readonly onTop = new DisposableStore();
	private onTopState: DeferredPromise<unknown> | undefined;

	/**
	 * The services are public for the scripts that drive the window directly:
	 * the smoke fixtures and the latency harness.
	 */
	constructor(
		@IEngineService public readonly engine: IEngineService,
		@IDataChannelService public readonly dataChannels: IDataChannelService,
		@IPermissionService public readonly permissions: IPermissionService,
		@IMainProcessService public readonly mainProcess: IMainProcessService,
		@ICommandService public readonly commands: ICommandService,
		@IWorkbenchConfigurationService private readonly configuration: IWorkbenchConfigurationService,
		@INativeHostService private readonly nativeHost: INativeHostService,
	) { }

	public getWindowId(): Promise<number> {
		return Promise.resolve(this.nativeHost.windowId);
	}

	public async whenWorkbenchRestored(): Promise<void> {
		const root = mainWindow.document.documentElement;
		if (root.dataset.workbenchReady === 'true') {
			return;
		}
		const ready = new DeferredPromise<void>();
		const observer = new MutationObserver(() => {
			if (root.dataset.workbenchReady === 'true') {
				void ready.complete();
			}
		});
		observer.observe(root, { attributes: true, attributeFilter: ['data-workbench-ready'] });
		try {
			await within(ready.p, 10000, () => 'Workbench did not become ready');
		} finally {
			observer.disconnect();
		}
	}

	public setValue(selector: string, text: string): Promise<void> {
		return answer(() => {
			const element = mainWindow.document.querySelector(selector);
			if (!(element instanceof HTMLInputElement)) {
				throw new Error(`Element not found: ${selector}`);
			}
			element.value = text;
			const event = new Event('input', { bubbles: true, cancelable: true });
			element.dispatchEvent(event);
		});
	}

	public isActiveElement(selector: string): Promise<boolean> {
		return answer(() => {
			const element = mainWindow.document.querySelector(selector);
			if (element !== mainWindow.document.activeElement) {
				const chain: string[] = [];
				let el = mainWindow.document.activeElement;
				while (el) {
					const tagName = el.tagName;
					const id = el.id ? `#${el.id}` : '';
					const classes = coalesce(el.className.split(/\s+/g).map(c => c.trim())).map(c => `.${c}`).join('');
					chain.unshift(`${tagName}${id}${classes}`);
					el = el.parentElement;
				}
				throw new Error(`Active element not found. Current active element is '${chain.join(' > ')}'. Looking for ${selector}`);
			}
			return true;
		});
	}

	public getElements(selector: string, recursive: boolean): Promise<IElement[]> {
		return answer(() => {
			const query = mainWindow.document.querySelectorAll(selector);
			const result: IElement[] = [];
			for (let i = 0; i < query.length; i++) {
				const element = query.item(i);
				result.push(this.serializeElement(element, recursive));
			}
			return result;
		});
	}

	private serializeElement(element: Element, recursive: boolean): IElement {
		const attributes: Record<string, string> = {};
		for (let j = 0; j < element.attributes.length; j++) {
			const attr = element.attributes.item(j);
			if (attr) {
				attributes[attr.name] = attr.value;
			}
		}
		const children: IElement[] = [];
		if (recursive) {
			for (let i = 0; i < element.children.length; i++) {
				const child = element.children.item(i);
				if (child) {
					children.push(this.serializeElement(child, true));
				}
			}
		}

		// The document's root has no offset parent to walk: it sits at the origin.
		const placed = isHTMLElement(element) && element !== mainWindow.document.documentElement;
		const { left, top } = placed ? getTopLeftOffset(element) : { left: 0, top: 0 };
		return {
			tagName: element.tagName,
			className: element.className,
			textContent: element.textContent,
			attributes,
			children,
			left,
			top
		};
	}

	/** The `index`th element `selector` matches, or a failure that names the selector. */
	private element(selector: string, index = 0): HTMLElement {
		const element = mainWindow.document.querySelectorAll(selector).item(index);
		if (!isHTMLElement(element)) {
			throw new Error(`Element not found: ${selector}${index ? ` [${String(index)}]` : ''}`);
		}
		return element;
	}

	private layoutOf(element: HTMLElement, styles: readonly string[]): IElementLayout {
		const box = element.getBoundingClientRect();
		const computed = mainWindow.getComputedStyle(element);
		return {
			top: box.top, left: box.left, right: box.right, bottom: box.bottom, width: box.width, height: box.height,
			scrollTop: element.scrollTop, scrollWidth: element.scrollWidth, clientWidth: element.clientWidth,
			scrollHeight: element.scrollHeight, clientHeight: element.clientHeight,
			hidden: element.hidden === true, draggable: element.draggable,
			style: Object.fromEntries(styles.map(property => [property, computed.getPropertyValue(property)])),
		};
	}

	public getLayout(selector: string, styles: readonly string[] = []): Promise<IElementLayout> {
		return answer(() => this.layoutOf(this.element(selector), styles));
	}

	public getLayouts(selector: string, styles: readonly string[] = []): Promise<IElementLayout[]> {
		return answer(() => [...mainWindow.document.querySelectorAll(selector)].filter(isHTMLElement).map(element => this.layoutOf(element, styles)));
	}

	public scrollTo(selector: string, top: number): Promise<number> {
		return answer(() => {
			const element = this.element(selector);
			element.scrollTop = top;
			return element.scrollTop;
		});
	}

	/** A click as the person's pointer gives it, dispatched to the element. */
	public click(selector: string, index?: number): Promise<void> {
		return answer(() => {
			this.element(selector, index).dispatchEvent(new MouseEvent('click', { bubbles: true, cancelable: true }));
		});
	}

	public focus(selector: string, index?: number): Promise<void> {
		return answer(() => {
			this.element(selector, index).focus();
		});
	}

	public sendKey(key: string, keyCode: number, selector?: string, index?: number): Promise<void> {
		return answer(() => {
			const target = selector ? this.element(selector, index) : mainWindow;
			for (const type of ['keydown', 'keyup']) {
				target.dispatchEvent(new KeyboardEvent(type, { key, code: key, keyCode, bubbles: true }));
			}
		});
	}

	public insertText(selector: string, text: string, index?: number): Promise<void> {
		return answer(() => {
			this.element(selector, index).dispatchEvent(new InputEvent('beforeinput', { inputType: 'insertText', data: text, bubbles: true, cancelable: true }));
		});
	}

	public pointerDownOutside(): Promise<void> {
		return answer(() => {
			mainWindow.document.body.dispatchEvent(new PointerEvent('pointerdown', { bubbles: true }));
		});
	}

	public async whenAnimationsFinished(selector: string): Promise<void> {
		// A spinner runs for ever; only what ends is waited for.
		const ending = this.element(selector).getAnimations({ subtree: true }).filter(animation => animation.effect?.getComputedTiming().endTime !== Infinity);
		await Promise.all(ending.map(animation => animation.finished));
		await this.whenRendered();
	}

	public whenRendered(): Promise<void> {
		return new Promise(resolve => mainWindow.requestAnimationFrame(() => {
			mainWindow.requestAnimationFrame(() => {
				resolve();
			});
		}));
	}

	public mark(selector: string, name: string): Promise<void> {
		return answer(() => {
			this.element(selector).dataset.smokeMark = name;
		});
	}

	public async rendererExposure(): Promise<IRendererExposure> {
		// The preload sends only on `vnilink:` channels and refuses any other.
		let unprefixedRejected = false;
		try {
			ipcRenderer.send('engine:command', { type: 'snapshot' });
		} catch {
			unprefixedRejected = true;
		}
		const api: unknown = Reflect.get(mainWindow, 'vnilink');
		return {
			configuration: await context.resolveConfiguration(),
			nodeExposed: typeof Reflect.get(globalThis, 'require') !== 'undefined' || typeof Reflect.get(globalThis, 'process') !== 'undefined',
			vscodeExposed: 'vscode' in mainWindow,
			unprefixedRejected,
			api: typeof api === 'object' && api !== null ? Object.keys(api).sort() : [],
			windowOpened: mainWindow.open('about:blank') !== null,
		};
	}

	public execute(command: EngineCommand): Promise<EngineReply> {
		return this.engine.execute(command);
	}

	public updateSetting(key: string, value: unknown): Promise<void> {
		return this.configuration.updateValue(key, value);
	}

	public callMainProcess(channel: string, command: string, arg?: unknown): Promise<unknown> {
		return this.mainProcess.getChannel(channel).call(command, arg);
	}

	public permissionStatus(): Promise<unknown> {
		return this.permissions.execute({ type: 'status' });
	}

	public listenForEngineRestart(): void {
		this.restart.clear();
		const story: string[] = [];
		const back = new DeferredPromise<void>();
		this.restart.add(this.engine.onDidChangeRestarting(restarting => story.push(restarting ? 'restarting' : 'steady')));
		this.restart.add(this.engine.onDidChangeAvailability(available => {
			story.push(available ? 'available' : 'unavailable');
			if (available) {
				void back.complete();
			}
		}));
		this.restartStory = { story, back };
	}

	public async engineRestarted(): Promise<IEngineRestartStory> {
		const listening = this.restartStory;
		if (!listening) {
			throw new Error('Nothing listened for the engine restarting');
		}
		try {
			await within(listening.back.p, 30000, () => `The engine did not come back: ${listening.story.join(', ')}`);
			return { story: [...listening.story], restarting: this.engine.restarting };
		} finally {
			this.restart.clear();
			this.restartStory = undefined;
		}
	}

	public async whenHosting(hosting: boolean): Promise<void> {
		const heard = new DeferredPromise<void>();
		const listener = this.engine.onDidReceiveEvents(batch => {
			if (batch.events.some(event => event.type === 'hostingChanged' && event.hosting === hosting)) {
				void heard.complete();
			}
		});
		try {
			await within(heard.p, 5000, () => `No event said hosting turned ${hosting ? 'on' : 'off'}`);
		} finally {
			listener.dispose();
		}
	}

	public listenForAlwaysOnTop(): void {
		this.onTop.clear();
		const heard = new DeferredPromise<unknown>();
		this.onTop.add(this.nativeHost.onDidChangeWindowState(state => {
			if (state.alwaysOnTop) {
				void heard.complete(state);
			}
		}));
		this.onTopState = heard;
	}

	public async alwaysOnTop(): Promise<unknown> {
		const heard = this.onTopState;
		if (!heard) {
			throw new Error('Nothing listened for the window state');
		}
		try {
			return await within(heard.p, 5000, () => 'Window state event not received');
		} finally {
			this.onTop.clear();
			this.onTopState = undefined;
		}
	}
}

export function registerWindowDriver(instantiationService: IInstantiationService): void {
	Object.assign(mainWindow, { driver: instantiationService.createInstance(BrowserWindowDriver) });
}
