/*---------------------------------------------------------------------------------------------
 *  Copyright (c) Microsoft Corporation. All rights reserved.
 *  Licensed under the MIT License. See LICENSE.vscode.txt in the project root for license information.
 *--------------------------------------------------------------------------------------------*/

import { IKeyboardEvent, StandardKeyboardEvent } from './keyboardEvent.js';
import { IMouseEvent, StandardMouseEvent } from './mouseEvent.js';
import { IntervalTimer, _runWhenIdle, IdleDeadline } from '../common/async.js';
import { onUnexpectedError } from '../common/errors.js';
import * as event from '../common/event.js';
import { KeyCode } from '../common/keyCodes.js';
import { Disposable, DisposableStore, IDisposable, markAsSingleton, toDisposable } from '../common/lifecycle.js';
import { RemoteAuthorities } from '../common/network.js';
import * as platform from '../common/platform.js';
import { URI } from '../common/uri.js';
import { hash } from '../common/hash.js';
import { CodeWindow, ensureCodeWindow, mainWindow } from './window.js';

export interface IRegisteredCodeWindow {
	readonly window: CodeWindow;
	readonly disposables: DisposableStore;
}

//# region Multi-Window Support Utilities

export const {
	registerWindow,
	getWindow,
	getDocument,
	getWindows,
	getWindowsCount,
	getWindowId,
	getWindowById,
	hasWindow,
	onDidRegisterWindow,
	onWillUnregisterWindow,
	onDidUnregisterWindow
} = (function () {
	const windows = new Map<number, IRegisteredCodeWindow>();

	ensureCodeWindow(mainWindow, 1);
	const mainWindowRegistration = { window: mainWindow, disposables: new DisposableStore() };
	windows.set(mainWindow.vscodeWindowId, mainWindowRegistration);

	const onDidRegisterWindow = new event.Emitter<IRegisteredCodeWindow>();
	const onDidUnregisterWindow = new event.Emitter<CodeWindow>();
	const onWillUnregisterWindow = new event.Emitter<CodeWindow>();

	function getWindowById(windowId: number): IRegisteredCodeWindow | undefined;
	function getWindowById(windowId: number | undefined, fallbackToMain: true): IRegisteredCodeWindow;
	function getWindowById(windowId: number | undefined, fallbackToMain?: boolean): IRegisteredCodeWindow | undefined {
		const window = typeof windowId === 'number' ? windows.get(windowId) : undefined;

		return window ?? (fallbackToMain ? mainWindowRegistration : undefined);
	}

	return {
		onDidRegisterWindow: onDidRegisterWindow.event,
		onWillUnregisterWindow: onWillUnregisterWindow.event,
		onDidUnregisterWindow: onDidUnregisterWindow.event,
		registerWindow(window: CodeWindow): IDisposable {
			if (windows.has(window.vscodeWindowId)) {
				return Disposable.None;
			}

			const disposables = new DisposableStore();

			const registeredWindow = {
				window,
				disposables: disposables.add(new DisposableStore())
			};
			windows.set(window.vscodeWindowId, registeredWindow);

			disposables.add(toDisposable(() => {
				windows.delete(window.vscodeWindowId);
				onDidUnregisterWindow.fire(window);
			}));

			disposables.add(addDisposableListener(window, EventType.BEFORE_UNLOAD, () => {
				onWillUnregisterWindow.fire(window);
			}));

			onDidRegisterWindow.fire(registeredWindow);

			return disposables;
		},
		getWindows(): Iterable<IRegisteredCodeWindow> {
			return windows.values();
		},
		getWindowsCount(): number {
			return windows.size;
		},
		getWindowId(targetWindow: Window): number {
			return (targetWindow as CodeWindow).vscodeWindowId;
		},
		hasWindow(windowId: number): boolean {
			return windows.has(windowId);
		},
		getWindowById,
		getWindow(e: Node | UIEvent | undefined | null): CodeWindow {
			const candidateNode = e as Node | undefined | null;
			if (candidateNode?.ownerDocument?.defaultView) {
				return candidateNode.ownerDocument.defaultView.window as CodeWindow;
			}

			const candidateEvent = e as UIEvent | undefined | null;
			if (candidateEvent?.view) {
				return candidateEvent.view.window as CodeWindow;
			}

			return mainWindow;
		},
		getDocument(e: Node | UIEvent | undefined | null): Document {
			const candidateNode = e as Node | undefined | null;
			return getWindow(candidateNode).document;
		}
	};
})();

//#endregion

//#region External Focus Tracking

/**
 * Information about external focus state, including the associated window.
 */
export interface IExternalFocusInfo {
	readonly hasFocus: boolean;
	readonly window?: CodeWindow;
}

/**
 * A function that checks if a component outside the normal DOM tree has focus.
 * Returns focus info including which window the component is associated with.
 */
export type ExternalFocusChecker = () => IExternalFocusInfo;

/**
 * A registry for functions that check if a component outside the normal DOM tree has focus.
 * This is used to extend the concept of "window has focus" to include things like
 * Electron WebContentsViews (browser views) that exist outside the workbench DOM.
 */
const externalFocusCheckers = new Set<ExternalFocusChecker>();

/**
 * Get the window associated with a focused external component.
 * This is used to determine which window should receive UI like dialogs
 * when an external component (like a browser view) has focus.
 *
 * @returns The window of the focused external component, or undefined if none
 */
export function getExternalFocusWindow(): CodeWindow | undefined {
	for (const checker of externalFocusCheckers) {
		const info = checker();
		if (info.hasFocus && info.window) {
			return info.window;
		}
	}
	return undefined;
}

//#endregion

export function clearNode(node: HTMLElement): void {
	while (node.firstChild) {
		node.firstChild.remove();
	}
}

class DomListener implements IDisposable {

	private _handler: (e: any) => void;
	private _node: EventTarget;
	private readonly _type: string;
	private readonly _options: boolean | AddEventListenerOptions;

	constructor(node: EventTarget, type: string, handler: (e: any) => void, options?: boolean | AddEventListenerOptions) {
		this._node = node;
		this._type = type;
		this._handler = handler;
		this._options = (options || false);
		this._node.addEventListener(this._type, this._handler, this._options);
	}

	dispose(): void {
		if (!this._handler) {
			// Already disposed
			return;
		}

		this._node.removeEventListener(this._type, this._handler, this._options);

		// Prevent leakers from holding on to the dom or handler func
		this._node = null!;
		this._handler = null!;
	}
}

export function addDisposableListener<K extends keyof GlobalEventHandlersEventMap>(node: EventTarget, type: K, handler: (event: GlobalEventHandlersEventMap[K]) => void, useCapture?: boolean): IDisposable;
export function addDisposableListener(node: EventTarget, type: string, handler: (event: any) => void, useCapture?: boolean): IDisposable;
export function addDisposableListener(node: EventTarget, type: string, handler: (event: any) => void, options: AddEventListenerOptions): IDisposable;
export function addDisposableListener(node: EventTarget, type: string, handler: (event: any) => void, useCaptureOrOptions?: boolean | AddEventListenerOptions): IDisposable {
	return new DomListener(node, type, handler, useCaptureOrOptions);
}

export interface IAddStandardDisposableListenerSignature {
	(node: HTMLElement | Element | Document, type: 'click', handler: (event: IMouseEvent) => void, useCapture?: boolean): IDisposable;
	(node: HTMLElement | Element | Document, type: 'mousedown', handler: (event: IMouseEvent) => void, useCapture?: boolean): IDisposable;
	(node: HTMLElement | Element | Document, type: 'keydown', handler: (event: IKeyboardEvent) => void, useCapture?: boolean): IDisposable;
	(node: HTMLElement | Element | Document, type: 'keypress', handler: (event: IKeyboardEvent) => void, useCapture?: boolean): IDisposable;
	(node: HTMLElement | Element | Document, type: 'keyup', handler: (event: IKeyboardEvent) => void, useCapture?: boolean): IDisposable;
	(node: HTMLElement | Element | Document, type: 'pointerdown', handler: (event: PointerEvent) => void, useCapture?: boolean): IDisposable;
	(node: HTMLElement | Element | Document, type: 'pointermove', handler: (event: PointerEvent) => void, useCapture?: boolean): IDisposable;
	(node: HTMLElement | Element | Document, type: 'pointerup', handler: (event: PointerEvent) => void, useCapture?: boolean): IDisposable;
	(node: HTMLElement | Element | Document, type: string, handler: (event: any) => void, useCapture?: boolean): IDisposable;
}
function _wrapAsStandardMouseEvent(targetWindow: Window, handler: (e: IMouseEvent) => void): (e: MouseEvent) => void {
	return function (e: MouseEvent) {
		return handler(new StandardMouseEvent(targetWindow, e));
	};
}
function _wrapAsStandardKeyboardEvent(handler: (e: IKeyboardEvent) => void): (e: KeyboardEvent) => void {
	return function (e: KeyboardEvent) {
		return handler(new StandardKeyboardEvent(e));
	};
}
export const addStandardDisposableListener: IAddStandardDisposableListenerSignature = function addStandardDisposableListener(node: HTMLElement | Element | Document, type: string, handler: (event: any) => void, useCapture?: boolean): IDisposable {
	let wrapHandler = handler;

	if (type === 'click' || type === 'mousedown' || type === 'contextmenu') {
		wrapHandler = _wrapAsStandardMouseEvent(getWindow(node), handler);
	} else if (type === 'keydown' || type === 'keypress' || type === 'keyup') {
		wrapHandler = _wrapAsStandardKeyboardEvent(handler);
	}

	return addDisposableListener(node, type, wrapHandler, useCapture);
};

/**
 * Execute the callback the next time the browser is idle, returning an
 * {@link IDisposable} that will cancel the callback when disposed. This wraps
 * [requestIdleCallback] so it will fallback to [setTimeout] if the environment
 * doesn't support it.
 *
 * @param targetWindow The window for which to run the idle callback
 * @param callback The callback to run when idle, this includes an
 * [IdleDeadline] that provides the time alloted for the idle callback by the
 * browser. Not respecting this deadline will result in a degraded user
 * experience.
 * @param timeout A timeout at which point to queue no longer wait for an idle
 * callback but queue it on the regular event loop (like setTimeout). Typically
 * this should not be used.
 *
 * [IdleDeadline]: https://developer.mozilla.org/en-US/docs/Web/API/IdleDeadline
 * [requestIdleCallback]: https://developer.mozilla.org/en-US/docs/Web/API/Window/requestIdleCallback
 * [setTimeout]: https://developer.mozilla.org/en-US/docs/Web/API/Window/setTimeout
 */
export function runWhenWindowIdle(targetWindow: Window | typeof globalThis, callback: (idle: IdleDeadline) => void, timeout?: number): IDisposable {
	return _runWhenIdle(targetWindow, callback, timeout);
}

/**
 * Schedule a callback to be run at the next animation frame.
 * This allows multiple parties to register callbacks that should run at the next animation frame.
 * If currently in an animation frame, `runner` will be executed immediately.
 * @return token that can be used to cancel the scheduled runner (only if `runner` was not executed immediately).
 */
export let runAtThisOrScheduleAtNextAnimationFrame: (targetWindow: Window, runner: () => void, priority?: number) => IDisposable;
/**
 * Schedule a callback to be run at the next animation frame.
 * This allows multiple parties to register callbacks that should run at the next animation frame.
 * If currently in an animation frame, `runner` will be executed at the next animation frame.
 * @return token that can be used to cancel the scheduled runner.
 */
export let scheduleAtNextAnimationFrame: (targetWindow: Window, runner: () => void, priority?: number) => IDisposable;

export class WindowIntervalTimer extends IntervalTimer {

	private readonly defaultTarget?: Window & typeof globalThis;

	/**
	 *
	 * @param node The optional node from which the target window is determined
	 */
	constructor(node?: Node) {
		super();
		this.defaultTarget = node && getWindow(node);
	}

	override cancelAndSet(runner: () => void, interval: number, targetWindow?: Window & typeof globalThis): void {
		return super.cancelAndSet(runner, interval, targetWindow ?? this.defaultTarget);
	}
}

class AnimationFrameQueueItem implements IDisposable {

	private _runner: () => void;
	public priority: number;
	private _canceled: boolean;

	constructor(runner: () => void, priority: number = 0) {
		this._runner = runner;
		this.priority = priority;
		this._canceled = false;
	}

	dispose(): void {
		this._canceled = true;
	}

	execute(): void {
		if (this._canceled) {
			return;
		}

		try {
			this._runner();
		} catch (e) {
			onUnexpectedError(e);
		}
	}

	// Sort by priority (largest to lowest)
	static sort(a: AnimationFrameQueueItem, b: AnimationFrameQueueItem): number {
		return b.priority - a.priority;
	}
}

(function () {
	/**
	 * The runners scheduled at the next animation frame
	 */
	const NEXT_QUEUE = new Map<number /* window ID */, AnimationFrameQueueItem[]>();
	/**
	 * The runners scheduled at the current animation frame
	 */
	const CURRENT_QUEUE = new Map<number /* window ID */, AnimationFrameQueueItem[]>();
	/**
	 * A flag to keep track if the native requestAnimationFrame was already called
	 */
	const animFrameRequested = new Map<number /* window ID */, boolean>();
	/**
	 * A flag to indicate if currently handling a native requestAnimationFrame callback
	 */
	const inAnimationFrameRunner = new Map<number /* window ID */, boolean>();

	markAsSingleton(onDidUnregisterWindow(({ vscodeWindowId }) => {
		// A callback can close its window while the current queue is being processed.
		const currentQueue = CURRENT_QUEUE.get(vscodeWindowId);
		if (currentQueue) {
			currentQueue.length = 0;
		}
		NEXT_QUEUE.delete(vscodeWindowId);
		CURRENT_QUEUE.delete(vscodeWindowId);
		animFrameRequested.delete(vscodeWindowId);
		inAnimationFrameRunner.delete(vscodeWindowId);
	}));

	const animationFrameRunner = (targetWindowId: number) => {
		if (!animFrameRequested.has(targetWindowId)) {
			return;
		}
		animFrameRequested.set(targetWindowId, false);

		const currentQueue = NEXT_QUEUE.get(targetWindowId) ?? [];
		CURRENT_QUEUE.set(targetWindowId, currentQueue);
		NEXT_QUEUE.set(targetWindowId, []);

		inAnimationFrameRunner.set(targetWindowId, true);
		while (currentQueue.length > 0) {
			currentQueue.sort(AnimationFrameQueueItem.sort);
			const top = currentQueue.shift()!;
			top.execute();
		}
		if (inAnimationFrameRunner.has(targetWindowId)) {
			inAnimationFrameRunner.set(targetWindowId, false);
		}
	};

	scheduleAtNextAnimationFrame = (targetWindow: Window, runner: () => void, priority: number = 0) => {
		const targetWindowId = getWindowId(targetWindow);
		const item = new AnimationFrameQueueItem(runner, priority);

		let nextQueue = NEXT_QUEUE.get(targetWindowId);
		if (!nextQueue) {
			nextQueue = [];
			NEXT_QUEUE.set(targetWindowId, nextQueue);
		}
		nextQueue.push(item);

		if (!animFrameRequested.get(targetWindowId)) {
			animFrameRequested.set(targetWindowId, true);
			targetWindow.requestAnimationFrame(() => animationFrameRunner(targetWindowId));
		}

		return item;
	};

	runAtThisOrScheduleAtNextAnimationFrame = (targetWindow: Window, runner: () => void, priority?: number) => {
		const targetWindowId = getWindowId(targetWindow);
		if (inAnimationFrameRunner.get(targetWindowId)) {
			const item = new AnimationFrameQueueItem(runner, priority);
			let currentQueue = CURRENT_QUEUE.get(targetWindowId);
			if (!currentQueue) {
				currentQueue = [];
				CURRENT_QUEUE.set(targetWindowId, currentQueue);
			}
			currentQueue.push(item);
			return item;
		} else {
			return scheduleAtNextAnimationFrame(targetWindow, runner, priority);
		}
	};
})();

export function getComputedStyle(el: HTMLElement): CSSStyleDeclaration {
	return getWindow(el).getComputedStyle(el, null);
}

export function getClientArea(element: HTMLElement, defaultValue?: Dimension, fallbackElement?: HTMLElement): Dimension {
	const elWindow = getWindow(element);
	const elDocument = elWindow.document;

	// Try with DOM clientWidth / clientHeight
	if (element !== elDocument.body) {
		return new Dimension(element.clientWidth, element.clientHeight);
	}

	// If visual view port exits and it's on mobile, it should be used instead of window innerWidth / innerHeight, or document.body.clientWidth / document.body.clientHeight
	if (platform.isIOS && elWindow?.visualViewport) {
		return new Dimension(elWindow.visualViewport.width, elWindow.visualViewport.height);
	}

	// Try innerWidth / innerHeight
	if (elWindow?.innerWidth && elWindow.innerHeight) {
		return new Dimension(elWindow.innerWidth, elWindow.innerHeight);
	}

	// Try with document.body.clientWidth / document.body.clientHeight
	if (elDocument.body && elDocument.body.clientWidth && elDocument.body.clientHeight) {
		return new Dimension(elDocument.body.clientWidth, elDocument.body.clientHeight);
	}

	// Try with document.documentElement.clientWidth / document.documentElement.clientHeight
	if (elDocument.documentElement && elDocument.documentElement.clientWidth && elDocument.documentElement.clientHeight) {
		return new Dimension(elDocument.documentElement.clientWidth, elDocument.documentElement.clientHeight);
	}

	if (fallbackElement) {
		return getClientArea(fallbackElement, defaultValue);
	}

	if (defaultValue) {
		return defaultValue;
	}

	throw new Error('Unable to figure out browser width and height');
}

class SizeUtils {
	// Adapted from WinJS
	// Converts a CSS positioning string for the specified element to pixels.
	private static convertToPixels(element: HTMLElement, value: string): number {
		return parseFloat(value) || 0;
	}

	private static getDimension(element: HTMLElement, cssPropertyName: string): number {
		const computedStyle = getComputedStyle(element);
		const value = computedStyle ? computedStyle.getPropertyValue(cssPropertyName) : '0';
		return SizeUtils.convertToPixels(element, value);
	}

	static getBorderLeftWidth(element: HTMLElement): number {
		return SizeUtils.getDimension(element, 'border-left-width');
	}
	static getBorderRightWidth(element: HTMLElement): number {
		return SizeUtils.getDimension(element, 'border-right-width');
	}
	static getBorderTopWidth(element: HTMLElement): number {
		return SizeUtils.getDimension(element, 'border-top-width');
	}
	static getBorderBottomWidth(element: HTMLElement): number {
		return SizeUtils.getDimension(element, 'border-bottom-width');
	}

	static getPaddingLeft(element: HTMLElement): number {
		return SizeUtils.getDimension(element, 'padding-left');
	}
	static getPaddingRight(element: HTMLElement): number {
		return SizeUtils.getDimension(element, 'padding-right');
	}
	static getPaddingTop(element: HTMLElement): number {
		return SizeUtils.getDimension(element, 'padding-top');
	}
	static getPaddingBottom(element: HTMLElement): number {
		return SizeUtils.getDimension(element, 'padding-bottom');
	}

	static getMarginLeft(element: HTMLElement): number {
		return SizeUtils.getDimension(element, 'margin-left');
	}
	static getMarginTop(element: HTMLElement): number {
		return SizeUtils.getDimension(element, 'margin-top');
	}
	static getMarginRight(element: HTMLElement): number {
		return SizeUtils.getDimension(element, 'margin-right');
	}
	static getMarginBottom(element: HTMLElement): number {
		return SizeUtils.getDimension(element, 'margin-bottom');
	}
}

// ----------------------------------------------------------------------------------------
// Position & Dimension

export interface IDimension {
	readonly width: number;
	readonly height: number;
}

export class Dimension implements IDimension {

	constructor(
		readonly width: number,
		readonly height: number,
	) { }

	static lift(obj: IDimension): Dimension {
		if (obj instanceof Dimension) {
			return obj;
		} else {
			return new Dimension(obj.width, obj.height);
		}
	}
}

export interface IDomPosition {
	readonly left: number;
	readonly top: number;
}

export function getTopLeftOffset(element: HTMLElement): IDomPosition {
	// Adapted from WinJS.Utilities.getPosition
	// and added borders to the mix

	let offsetParent = element.offsetParent;
	let top = element.offsetTop;
	let left = element.offsetLeft;

	while (
		(element = <HTMLElement>element.parentNode) !== null
		&& element !== element.ownerDocument.body
		&& element !== element.ownerDocument.documentElement
	) {
		top -= element.scrollTop;
		const c = isShadowRoot(element) ? null : getComputedStyle(element);
		if (c) {
			left -= c.direction !== 'rtl' ? element.scrollLeft : -element.scrollLeft;
		}

		if (element === offsetParent) {
			left += SizeUtils.getBorderLeftWidth(element);
			top += SizeUtils.getBorderTopWidth(element);
			top += element.offsetTop;
			left += element.offsetLeft;
			offsetParent = element.offsetParent;
		}
	}

	return {
		left: left,
		top: top
	};
}

export interface IDomNodePagePosition {
	left: number;
	top: number;
	width: number;
	height: number;
}

/**
 * Returns the position of a dom node relative to the entire page.
 */
export function getDomNodePagePosition(domNode: HTMLElement): IDomNodePagePosition {
	const bb = domNode.getBoundingClientRect();
	const window = getWindow(domNode);
	return {
		left: bb.left + window.scrollX,
		top: bb.top + window.scrollY,
		width: bb.width,
		height: bb.height
	};
}

/**
 * Returns the effective zoom on a given element before window zoom level is applied
 */
export function getDomNodeZoomLevel(domNode: HTMLElement): number {
	let testElement: HTMLElement | null = domNode;
	let zoom = 1.0;
	do {
		// eslint-disable-next-line local/code-no-any-casts
		const elementZoomLevel = (getComputedStyle(testElement) as any).zoom;
		if (elementZoomLevel !== null && elementZoomLevel !== undefined && elementZoomLevel !== '1') {
			zoom *= elementZoomLevel;
		}

		testElement = testElement.parentElement;
	} while (testElement !== null && testElement !== testElement.ownerDocument.documentElement);

	return zoom;
}


// Adapted from WinJS
// Gets the width of the element, including margins.
export function getTotalWidth(element: HTMLElement): number {
	const margin = SizeUtils.getMarginLeft(element) + SizeUtils.getMarginRight(element);
	return element.offsetWidth + margin;
}

export function getContentWidth(element: HTMLElement): number {
	const border = SizeUtils.getBorderLeftWidth(element) + SizeUtils.getBorderRightWidth(element);
	const padding = SizeUtils.getPaddingLeft(element) + SizeUtils.getPaddingRight(element);
	return element.offsetWidth - border - padding;
}

// Adapted from WinJS
// Gets the height of the content of the specified element. The content height does not include borders or padding.
export function getContentHeight(element: HTMLElement): number {
	const border = SizeUtils.getBorderTopWidth(element) + SizeUtils.getBorderBottomWidth(element);
	const padding = SizeUtils.getPaddingTop(element) + SizeUtils.getPaddingBottom(element);
	return element.offsetHeight - border - padding;
}

// Adapted from WinJS
// Gets the height of the element, including its margins.
export function getTotalHeight(element: HTMLElement): number {
	const margin = SizeUtils.getMarginTop(element) + SizeUtils.getMarginBottom(element);
	return element.offsetHeight + margin;
}

// ----------------------------------------------------------------------------------------

export function isAncestor(testChild: Node | null, testAncestor: Node | null): boolean {
	return Boolean(testAncestor?.contains(testChild));
}

export function isShadowRoot(node: Node): node is ShadowRoot {
	return (
		node && !!(<ShadowRoot>node).host && !!(<ShadowRoot>node).mode
	);
}

export function getShadowRoot(domNode: Node): ShadowRoot | null {
	while (domNode.parentNode) {
		if (domNode === domNode.ownerDocument?.body) {
			// reached the body
			return null;
		}
		domNode = domNode.parentNode;
	}
	return isShadowRoot(domNode) ? domNode : null;
}

/**
 * Returns the active element across all child windows
 * based on document focus. Falls back to the main
 * window if no window has focus.
 */
export function getActiveElement(): Element | null {
	let result = getActiveDocument().activeElement;

	while (result?.shadowRoot) {
		result = result.shadowRoot.activeElement;
	}

	return result;
}

/**
 * Returns true if the focused window active element matches
 * the provided element. Falls back to the main window if no
 * window has focus.
 */
export function isActiveElement(element: Element): boolean {
	return getActiveElement() === element;
}

/**
 * Returns the active document across main and child windows.
 * Prefers the window with focus (including external components like browser views),
 * otherwise falls back to the main windows document.
 */
export function getActiveDocument(): Document {
	if (getWindowsCount() <= 1) {
		return mainWindow.document;
	}

	const documents = Array.from(getWindows()).map(({ window }) => window.document);
	const focusedDoc = documents.find(doc => doc.hasFocus());
	if (focusedDoc) {
		return focusedDoc;
	}

	// Check if an external component (like browser view) has focus
	const externalWindow = getExternalFocusWindow();
	if (externalWindow) {
		return externalWindow.document;
	}

	return mainWindow.document;
}

/**
 * Returns the active window across main and child windows.
 * Prefers the window with focus, otherwise falls back to
 * the main window.
 */
export function getActiveWindow(): CodeWindow {
	const document = getActiveDocument();
	return (document.defaultView?.window ?? mainWindow) as CodeWindow;
}

interface IMutationObserver {
	users: number;
	readonly observer: MutationObserver;
	readonly onDidMutate: event.Event<MutationRecord[]>;
}

export const sharedMutationObserver = new class {

	readonly mutationObservers = new Map<Node, Map<number, IMutationObserver>>();

	observe(target: Node, disposables: DisposableStore, options?: MutationObserverInit): event.Event<MutationRecord[]> {
		let mutationObserversPerTarget = this.mutationObservers.get(target);
		if (!mutationObserversPerTarget) {
			mutationObserversPerTarget = new Map<number, IMutationObserver>();
			this.mutationObservers.set(target, mutationObserversPerTarget);
		}

		const optionsHash = hash(options);
		let mutationObserverPerOptions = mutationObserversPerTarget.get(optionsHash);
		if (!mutationObserverPerOptions) {
			const onDidMutate = new event.Emitter<MutationRecord[]>();
			const observer = new MutationObserver(mutations => onDidMutate.fire(mutations));
			observer.observe(target, options);

			const resolvedMutationObserverPerOptions = mutationObserverPerOptions = {
				users: 1,
				observer,
				onDidMutate: onDidMutate.event
			};

			disposables.add(toDisposable(() => {
				resolvedMutationObserverPerOptions.users -= 1;

				if (resolvedMutationObserverPerOptions.users === 0) {
					onDidMutate.dispose();
					observer.disconnect();

					mutationObserversPerTarget?.delete(optionsHash);
					if (mutationObserversPerTarget?.size === 0) {
						this.mutationObservers.delete(target);
					}
				}
			}));

			mutationObserversPerTarget.set(optionsHash, mutationObserverPerOptions);
		} else {
			mutationObserverPerOptions.users += 1;
		}

		return mutationObserverPerOptions.onDidMutate;
	}
};

export function isHTMLElement(e: unknown): e is HTMLElement {
	// eslint-disable-next-line no-restricted-syntax
	return e instanceof HTMLElement || e instanceof getWindow(e as Node).HTMLElement;
}

export function isSVGElement(e: unknown): e is SVGElement {
	// eslint-disable-next-line no-restricted-syntax
	return e instanceof SVGElement || e instanceof getWindow(e as Node).SVGElement;
}

export function isMouseEvent(e: unknown): e is MouseEvent {
	// eslint-disable-next-line no-restricted-syntax
	return e instanceof MouseEvent || e instanceof getWindow(e as UIEvent).MouseEvent;
}

export const EventType = {
	// Mouse
	CLICK: 'click',
	AUXCLICK: 'auxclick',
	DBLCLICK: 'dblclick',
	MOUSE_UP: 'mouseup',
	MOUSE_DOWN: 'mousedown',
	MOUSE_OVER: 'mouseover',
	MOUSE_MOVE: 'mousemove',
	MOUSE_OUT: 'mouseout',
	MOUSE_ENTER: 'mouseenter',
	MOUSE_LEAVE: 'mouseleave',
	MOUSE_WHEEL: 'wheel',
	POINTER_UP: 'pointerup',
	POINTER_DOWN: 'pointerdown',
	POINTER_MOVE: 'pointermove',
	CONTEXT_MENU: 'contextmenu',
	// Keyboard
	KEY_DOWN: 'keydown',
	KEY_UP: 'keyup',
	BEFORE_UNLOAD: 'beforeunload',
	PASTE: 'paste',
	RESIZE: 'resize',
	SCROLL: 'scroll',
	CHANGE: 'change',
	FOCUS: 'focus',
	FOCUS_IN: 'focusin',
	FOCUS_OUT: 'focusout',
	BLUR: 'blur',
	INPUT: 'input',
	// Drag
	DRAG_START: 'dragstart',
	DRAG_LEAVE: 'dragleave',
	DRAG_OVER: 'dragover',
	DROP: 'drop',
	DRAG_END: 'dragend'
} as const;

export interface EventLike {
	preventDefault(): void;
	stopPropagation(): void;
}

export const EventHelper = {
	stop: <T extends EventLike>(e: T, cancelBubble?: boolean): T => {
		e.preventDefault();
		if (cancelBubble) {
			e.stopPropagation();
		}
		return e;
	}
};

export interface IFocusTracker extends Disposable {
	readonly onDidFocus: event.Event<void>;
	readonly onDidBlur: event.Event<void>;
}

class FocusTracker extends Disposable implements IFocusTracker {

	private readonly _onDidFocus = this._register(new event.Emitter<void>());
	get onDidFocus() { return this._onDidFocus.event; }

	private readonly _onDidBlur = this._register(new event.Emitter<void>());
	get onDidBlur() { return this._onDidBlur.event; }

	private _refreshStateHandler: () => void;

	private static hasFocusWithin(element: HTMLElement | Window): boolean {
		if (isHTMLElement(element)) {
			const shadowRoot = getShadowRoot(element);
			const activeElement = (shadowRoot ? shadowRoot.activeElement : element.ownerDocument.activeElement);
			return isAncestor(activeElement, element);
		} else {
			const window = element;
			return isAncestor(window.document.activeElement, window.document);
		}
	}

	constructor(element: HTMLElement | Window) {
		super();
		let hasFocus = FocusTracker.hasFocusWithin(element);
		let loosingFocus = false;

		const onFocus = () => {
			loosingFocus = false;
			if (!hasFocus) {
				hasFocus = true;
				this._onDidFocus.fire();
			}
		};

		const onBlur = () => {
			if (hasFocus) {
				loosingFocus = true;
				(isHTMLElement(element) ? getWindow(element) : element).setTimeout(() => {
					if (loosingFocus) {
						loosingFocus = false;
						hasFocus = false;
						this._onDidBlur.fire();
					}
				}, 0);
			}
		};

		this._refreshStateHandler = () => {
			const currentNodeHasFocus = FocusTracker.hasFocusWithin(<HTMLElement>element);
			if (currentNodeHasFocus !== hasFocus) {
				if (hasFocus) {
					onBlur();
				} else {
					onFocus();
				}
			}
		};

		this._register(addDisposableListener(element, EventType.FOCUS, onFocus, true));
		this._register(addDisposableListener(element, EventType.BLUR, onBlur, true));
		if (isHTMLElement(element)) {
			this._register(addDisposableListener(element, EventType.FOCUS_IN, () => this._refreshStateHandler()));
			this._register(addDisposableListener(element, EventType.FOCUS_OUT, () => this._refreshStateHandler()));
		}

	}
}

/**
 * Creates a new `IFocusTracker` instance that tracks focus changes on the given `element` and its descendants.
 *
 * @param element The `HTMLElement` or `Window` to track focus changes on.
 * @returns An `IFocusTracker` instance.
 */
export function trackFocus(element: HTMLElement | Window): IFocusTracker {
	return new FocusTracker(element);
}

export function append<T extends Node>(parent: HTMLElement, child: T): T;
export function append<T extends Node>(parent: HTMLElement, ...children: (T | string)[]): void;
export function append<T extends Node>(parent: HTMLElement, ...children: (T | string)[]): T | void {
	parent.append(...children);
	if (children.length === 1 && typeof children[0] !== 'string') {
		return children[0];
	}
}

/**
 * Removes all children from `parent` and appends `children`
 */
export function reset(parent: HTMLElement, ...children: Array<Node | string>): void {
	parent.textContent = '';
	append(parent, ...children);
}

const SELECTOR_REGEX = /([\w\-]+)?(#([\w\-]+))?((\.([\w\-]+))*)/;

export enum Namespace {
	HTML = 'http://www.w3.org/1999/xhtml',
	SVG = 'http://www.w3.org/2000/svg'
}

function _$<T extends Element>(namespace: Namespace, description: string, attrs?: { [key: string]: any }, ...children: Array<Node | string>): T {
	const match = SELECTOR_REGEX.exec(description);

	if (!match) {
		throw new Error('Bad use of emmet');
	}

	const tagName = match[1] || 'div';
	let result: T;

	if (namespace !== Namespace.HTML) {
		result = document.createElementNS(namespace as string, tagName) as T;
	} else {
		result = document.createElement(tagName) as unknown as T;
	}

	if (match[3]) {
		result.id = match[3];
	}
	if (match[4]) {
		result.className = match[4].replace(/\./g, ' ').trim();
	}

	if (attrs) {
		Object.entries(attrs).forEach(([name, value]) => {
			if (typeof value === 'undefined') {
				return;
			}

			if (/^on\w+$/.test(name)) {
				// eslint-disable-next-line local/code-no-any-casts
				(<any>result)[name] = value;
			} else if (name === 'selected') {
				if (value) {
					result.setAttribute(name, 'true');
				}

			} else {
				result.setAttribute(name, value);
			}
		});
	}

	result.append(...children);

	return result;
}

export function $<T extends HTMLElement>(description: string, attrs?: { [key: string]: any }, ...children: Array<Node | string>): T {
	return _$(Namespace.HTML, description, attrs, ...children);
}

$.SVG = function <T extends SVGElement>(description: string, attrs?: { [key: string]: any }, ...children: Array<Node | string>): T {
	return _$(Namespace.SVG, description, attrs, ...children);
};

export function show(...elements: HTMLElement[]): void {
	for (const element of elements) {
		element.style.display = '';
		element.removeAttribute('aria-hidden');
	}
}

export function hide(...elements: HTMLElement[]): void {
	for (const element of elements) {
		element.style.display = 'none';
		element.setAttribute('aria-hidden', 'true');
	}
}

export function animate(targetWindow: Window, fn: () => void): IDisposable {
	const step = () => {
		fn();
		stepDisposable = scheduleAtNextAnimationFrame(targetWindow, step);
	};

	let stepDisposable = scheduleAtNextAnimationFrame(targetWindow, step);
	return toDisposable(() => stepDisposable.dispose());
}

RemoteAuthorities.setPreferredWebSchema(/^https:/.test(mainWindow.location.href) ? 'https' : 'http');

export function triggerDownload(dataOrUri: Uint8Array | URI, name: string): void {

	// If the data is provided as Buffer, we create a
	// blob URL out of it to produce a valid link
	let url: string;
	if (URI.isUri(dataOrUri)) {
		url = dataOrUri.toString(true);
	} else {
		const blob = new Blob([dataOrUri as Uint8Array<ArrayBuffer>]);
		url = URL.createObjectURL(blob);

		// Ensure to free the data from DOM eventually
		setTimeout(() => URL.revokeObjectURL(url));
	}

	// In order to download from the browser, the only way seems
	// to be creating a <a> element with download attribute that
	// points to the file to download.
	// See also https://developers.google.com/web/updates/2011/08/Downloading-resources-in-HTML5-a-download
	const activeWindow = getActiveWindow();
	const anchor = document.createElement('a');
	activeWindow.document.body.appendChild(anchor);
	anchor.download = name;
	anchor.href = url;
	anchor.click();

	// Ensure to remove the element from DOM eventually
	setTimeout(() => anchor.remove());
}

export function triggerUpload(): Promise<FileList | undefined> {
	return new Promise<FileList | undefined>(resolve => {

		// In order to upload to the browser, create a
		// input element of type `file` and click it
		// to gather the selected files
		const activeWindow = getActiveWindow();
		const input = document.createElement('input');
		activeWindow.document.body.appendChild(input);
		input.type = 'file';
		input.multiple = true;

		// Resolve once the input event has fired once
		event.Event.once(event.Event.fromDOMEventEmitter(input, 'input'))(() => {
			resolve(input.files ?? undefined);
		});

		input.click();

		// Ensure to remove the element from DOM eventually
		setTimeout(() => input.remove());
	});
}

type ModifierKey = 'alt' | 'ctrl' | 'shift' | 'meta';

export interface IModifierKeyStatus {
	altKey: boolean;
	shiftKey: boolean;
	ctrlKey: boolean;
	metaKey: boolean;
	lastKeyPressed?: ModifierKey;
	lastKeyReleased?: ModifierKey;
	/**
	 * The keyboard event that caused the change. Only available while
	 * listeners of {@link ModifierKeyEmitter} are notified.
	 */
	event?: KeyboardEvent;
}

export class ModifierKeyEmitter extends event.Emitter<IModifierKeyStatus> {

	// This emitter is a lazily created singleton (see `getInstance`) that is allowed
	// to outlive the test that happens to create it first. `Emitter` itself is not
	// tracked, so the store has to be marked to keep it out of leak detection.
	private readonly _subscriptions = markAsSingleton(new DisposableStore());
	private _keyStatus: IModifierKeyStatus;
	private static instance: ModifierKeyEmitter | undefined;

	private constructor() {
		super();

		this._keyStatus = {
			altKey: false,
			shiftKey: false,
			ctrlKey: false,
			metaKey: false
		};

		this._subscriptions.add(event.Event.runAndSubscribe(onDidRegisterWindow, ({ window, disposables }) => this.registerListeners(window, disposables), { window: mainWindow, disposables: this._subscriptions }));
	}

	private registerListeners(window: Window, disposables: DisposableStore): void {
		disposables.add(addDisposableListener(window, 'keydown', e => {
			if (e.defaultPrevented) {
				return;
			}

			const event = new StandardKeyboardEvent(e);
			// If Alt-key keydown event is repeated, ignore it #112347
			// Only known to be necessary for Alt-Key at the moment #115810
			if (event.keyCode === KeyCode.Alt && e.repeat) {
				return;
			}

			if (e.altKey && !this._keyStatus.altKey) {
				this._keyStatus.lastKeyPressed = 'alt';
			} else if (e.ctrlKey && !this._keyStatus.ctrlKey) {
				this._keyStatus.lastKeyPressed = 'ctrl';
			} else if (e.metaKey && !this._keyStatus.metaKey) {
				this._keyStatus.lastKeyPressed = 'meta';
			} else if (e.shiftKey && !this._keyStatus.shiftKey) {
				this._keyStatus.lastKeyPressed = 'shift';
			} else if (event.keyCode !== KeyCode.Alt) {
				this._keyStatus.lastKeyPressed = undefined;
			} else {
				return;
			}

			this._keyStatus.altKey = e.altKey;
			this._keyStatus.ctrlKey = e.ctrlKey;
			this._keyStatus.metaKey = e.metaKey;
			this._keyStatus.shiftKey = e.shiftKey;

			if (this._keyStatus.lastKeyPressed) {
				this.fireWithEvent(e);
			}
		}, true));

		disposables.add(addDisposableListener(window, 'keyup', e => {
			if (e.defaultPrevented) {
				return;
			}

			if (!e.altKey && this._keyStatus.altKey) {
				this._keyStatus.lastKeyReleased = 'alt';
			} else if (!e.ctrlKey && this._keyStatus.ctrlKey) {
				this._keyStatus.lastKeyReleased = 'ctrl';
			} else if (!e.metaKey && this._keyStatus.metaKey) {
				this._keyStatus.lastKeyReleased = 'meta';
			} else if (!e.shiftKey && this._keyStatus.shiftKey) {
				this._keyStatus.lastKeyReleased = 'shift';
			} else {
				this._keyStatus.lastKeyReleased = undefined;
			}

			if (this._keyStatus.lastKeyPressed !== this._keyStatus.lastKeyReleased) {
				this._keyStatus.lastKeyPressed = undefined;
			}

			this._keyStatus.altKey = e.altKey;
			this._keyStatus.ctrlKey = e.ctrlKey;
			this._keyStatus.metaKey = e.metaKey;
			this._keyStatus.shiftKey = e.shiftKey;

			if (this._keyStatus.lastKeyReleased) {
				this.fireWithEvent(e);
			}
		}, true));

		disposables.add(addDisposableListener(window.document.body, 'mousedown', () => {
			this._keyStatus.lastKeyPressed = undefined;
		}, true));

		disposables.add(addDisposableListener(window.document.body, 'mouseup', () => {
			this._keyStatus.lastKeyPressed = undefined;
		}, true));

		disposables.add(addDisposableListener(window.document.body, 'mousemove', e => {
			if (e.buttons) {
				this._keyStatus.lastKeyPressed = undefined;
			}
		}, true));

		disposables.add(addDisposableListener(window, 'blur', () => {
			this.resetKeyStatus();
		}));
	}

	get keyStatus(): IModifierKeyStatus {
		return this._keyStatus;
	}

	/**
	 * The keyboard event is only exposed while listeners are notified. Holding on to it
	 * would retain its target and event path, e.g. the DOM of an editor that was detached
	 * after the key press (#146841).
	 */
	private fireWithEvent(e: KeyboardEvent): void {
		const keyStatus = this._keyStatus;
		keyStatus.event = e;
		try {
			this.fire(keyStatus);
		} finally {
			keyStatus.event = undefined;
		}
	}

	/**
	 * Allows to explicitly reset the key status based on more knowledge (#109062)
	 */
	resetKeyStatus(): void {
		this.doResetKeyStatus();
		this.fire(this._keyStatus);
	}

	private doResetKeyStatus(): void {
		this._keyStatus = {
			altKey: false,
			shiftKey: false,
			ctrlKey: false,
			metaKey: false
		};
	}

	static getInstance() {
		if (!ModifierKeyEmitter.instance) {
			ModifierKeyEmitter.instance = new ModifierKeyEmitter();
		}

		return ModifierKeyEmitter.instance;
	}

	override dispose() {
		super.dispose();
		this._subscriptions.dispose();
	}
}

/**
 * A wrapper around `ResizeObserver` that is disposable.
 *
 * Behavior is intentionally identical to using `new ResizeObserver(callback)`
 * directly: the user-supplied callback runs synchronously inside the
 * browser's resize-observation phase, with the entries the browser delivered.
 * The wrapper adds three things on top:
 *
 * 1. Lifetime management: `dispose()` disconnects the underlying observer.
 * 2. Auxiliary-window support: pass `targetWindow` so the observer is
 *    constructed in the realm of the element being observed.
 * 3. Context for the
 *    `ResizeObserver loop completed with undelivered notifications` warning:
 *    each instance carries a stable `name`, and just before invoking the user
 *    callback we add that name to a bounded, per-window set that is cleared
 *    at the next animation frame. The warning is delivered as a stackless
 *    `ErrorEvent` on `window` after callbacks run, so error telemetry can
 *    include the wrapped observers that recently ran in that window (see
 *    {@link getRecentDisposableResizeObserverContextForLoopError}). This is
 *    delivery context, not causal attribution: the browser does not expose
 *    which observer or skipped target caused the warning.
 *
 * @param name Stable identifier used in loop-warning context. Prefer one that
 * survives minification and refactors (e.g. the consumer class + purpose)
 * since callstacks change across releases.
 * @param callback Invoked synchronously when the browser delivers resize
 * notifications, with the same entries the native `ResizeObserver` would
 * have delivered.
 * @param targetWindow The window whose `ResizeObserver` constructor should
 * be used. Defaults to `mainWindow`. Pass the containing window when
 * creating an observer for elements that live in an auxiliary window.
 * @param options Optional configuration. `resizeObserverCtor` is a test
 * seam that defaults to `targetWindow.ResizeObserver`.
 */
export class DisposableResizeObserver extends Disposable {

	private readonly observer: ResizeObserver;
	readonly name: string;

	constructor(
		name: string,
		callback: ResizeObserverCallback,
		targetWindow: CodeWindow = mainWindow,
		options?: { resizeObserverCtor?: typeof ResizeObserver },
	) {
		super();
		this.name = name;
		const ctor = options?.resizeObserverCtor ?? targetWindow.ResizeObserver;
		this.observer = new ctor((entries: ResizeObserverEntry[], observer) => {
			recordDisposableResizeObserverInvocation(targetWindow, this.name);
			try {
				callback(entries, observer);
			} catch (e) {
				onUnexpectedError(e);
			}
		});
		this._register(toDisposable(() => this.observer.disconnect()));
	}

	observe(target: Element, options?: ResizeObserverOptions): IDisposable {
		this.observer.observe(target, options);
		return toDisposable(() => this.observer.unobserve(target));
	}
}

/**
 * Keep the context bounded so a large delivery phase cannot create an
 * unbounded telemetry value. Names are static component identifiers, and are
 * sorted when read so equivalent phases share a stable bucket.
 */
const maxRecentDisposableResizeObservers = 8;

/**
 * Wrapped observers that ran recently in one window. This is deliberately
 * scoped by window because auxiliary windows have independent documents and
 * resize-observation delivery loops.
 */
interface IRecentDisposableResizeObserverContext {
	readonly names: Set<string>;
	overflow: boolean;
}

const recentDisposableResizeObserverContexts = new WeakMap<CodeWindow, IRecentDisposableResizeObserverContext>();

function recordDisposableResizeObserverInvocation(targetWindow: CodeWindow, name: string): void {
	let context = recentDisposableResizeObserverContexts.get(targetWindow);
	if (!context) {
		context = { names: new Set(), overflow: false };
		recentDisposableResizeObserverContexts.set(targetWindow, context);

		// ResizeObserver callbacks and the synthetic loop error are delivered
		// after requestAnimationFrame callbacks in the rendering update. A
		// request made here therefore clears this context at the next frame,
		// after telemetry has observed any warning from the current update.
		targetWindow.requestAnimationFrame(() => recentDisposableResizeObserverContexts.delete(targetWindow));
	}

	if (context.names.has(name)) {
		return;
	}
	if (context.names.size < maxRecentDisposableResizeObservers) {
		context.names.add(name);
	} else {
		context.overflow = true;
		const largestName = Array.from(context.names).sort().at(-1)!;
		if (name < largestName) {
			context.names.delete(largestName);
			context.names.add(name);
		}
	}
}

type HTMLElementAttributeKeys<T> = Partial<{ [K in keyof T]: T[K] extends Function ? never : T[K] extends object ? HTMLElementAttributeKeys<T[K]> : T[K] }>;
type ElementAttributes<T> = HTMLElementAttributeKeys<T> & Record<string, any>;
type RemoveHTMLElement<T> = T extends HTMLElement ? never : T;
type UnionToIntersection<U> = (U extends any ? (k: U) => void : never) extends ((k: infer I) => void) ? I : never;
type ArrayToObj<T extends readonly any[]> = UnionToIntersection<RemoveHTMLElement<T[number]>>;
type HHTMLElementTagNameMap = HTMLElementTagNameMap & { '': HTMLDivElement };

type TagToElement<T> = T extends `${infer TStart}#${string}`
	? TStart extends keyof HHTMLElementTagNameMap
	? HHTMLElementTagNameMap[TStart]
	: HTMLElement
	: T extends `${infer TStart}.${string}`
	? TStart extends keyof HHTMLElementTagNameMap
	? HHTMLElementTagNameMap[TStart]
	: HTMLElement
	: T extends keyof HTMLElementTagNameMap
	? HTMLElementTagNameMap[T]
	: HTMLElement;

type TagToElementAndId<TTag> = TTag extends `${infer TTag}@${infer TId}`
	? { element: TagToElement<TTag>; id: TId }
	: { element: TagToElement<TTag>; id: 'root' };

type TagToRecord<TTag> = TagToElementAndId<TTag> extends { element: infer TElement; id: infer TId }
	? Record<(TId extends string ? TId : never) | 'root', TElement>
	: never;

type Child = HTMLElement | string | Record<string, HTMLElement>;

const H_REGEX = /(?<tag>[\w\-]+)?(?:#(?<id>[\w\-]+))?(?<class>(?:\.(?:[\w\-]+))*)(?:@(?<name>(?:[\w\_])+))?/;

/**
 * A helper function to create nested dom nodes.
 *
 *
 * ```ts
 * const elements = h('div.code-view', [
 * 	h('div.title@title'),
 * 	h('div.container', [
 * 		h('div.gutter@gutterDiv'),
 * 		h('div@editor'),
 * 	]),
 * ]);
 * const editor = createEditor(elements.editor);
 * ```
*/
export function h<TTag extends string>
	(tag: TTag):
	TagToRecord<TTag> extends infer Y ? { [TKey in keyof Y]: Y[TKey] } : never;

export function h<TTag extends string, T extends Child[]>
	(tag: TTag, children: [...T]):
	(ArrayToObj<T> & TagToRecord<TTag>) extends infer Y ? { [TKey in keyof Y]: Y[TKey] } : never;

export function h<TTag extends string>
	(tag: TTag, attributes: Partial<ElementAttributes<TagToElement<TTag>>>):
	TagToRecord<TTag> extends infer Y ? { [TKey in keyof Y]: Y[TKey] } : never;

export function h<TTag extends string, T extends Child[]>
	(tag: TTag, attributes: Partial<ElementAttributes<TagToElement<TTag>>>, children: [...T]):
	(ArrayToObj<T> & TagToRecord<TTag>) extends infer Y ? { [TKey in keyof Y]: Y[TKey] } : never;

export function h(tag: string, ...args: [] | [attributes: { $: string } & Partial<ElementAttributes<HTMLElement>> | Record<string, any>, children?: any[]] | [children: any[]]): Record<string, HTMLElement> {
	let attributes: { $?: string } & Partial<ElementAttributes<HTMLElement>>;
	let children: (Record<string, HTMLElement> | HTMLElement)[] | undefined;

	if (Array.isArray(args[0])) {
		attributes = {};
		children = args[0];
	} else {
		// eslint-disable-next-line local/code-no-any-casts
		attributes = args[0] as any || {};
		children = args[1];
	}

	const match = H_REGEX.exec(tag);

	if (!match || !match.groups) {
		throw new Error('Bad use of h');
	}

	const tagName = match.groups['tag'] || 'div';
	const el = document.createElement(tagName);

	if (match.groups['id']) {
		el.id = match.groups['id'];
	}

	const classNames = [];
	if (match.groups['class']) {
		for (const className of match.groups['class'].split('.')) {
			if (className !== '') {
				classNames.push(className);
			}
		}
	}
	if (attributes.className !== undefined) {
		for (const className of attributes.className.split('.')) {
			if (className !== '') {
				classNames.push(className);
			}
		}
	}
	if (classNames.length > 0) {
		el.className = classNames.join(' ');
	}

	const result: Record<string, HTMLElement> = {};

	if (match.groups['name']) {
		result[match.groups['name']] = el;
	}

	if (children) {
		for (const c of children) {
			if (isHTMLElement(c)) {
				el.appendChild(c);
			} else if (typeof c === 'string') {
				el.append(c);
			} else if ('root' in c) {
				Object.assign(result, c);
				el.appendChild(c.root);
			}
		}
	}

	for (const [key, value] of Object.entries(attributes)) {
		if (key === 'className') {
			continue;
		} else if (key === 'style') {
			for (const [cssKey, cssValue] of Object.entries(value)) {
				el.style.setProperty(
					camelCaseToHyphenCase(cssKey),
					typeof cssValue === 'number' ? cssValue + 'px' : '' + cssValue
				);
			}
		} else if (key === 'tabIndex') {
			el.tabIndex = value;
		} else {
			el.setAttribute(camelCaseToHyphenCase(key), value.toString());
		}
	}

	result['root'] = el;

	return result;
}

function camelCaseToHyphenCase(str: string) {
	return str.replace(/([a-z])([A-Z])/g, '$1-$2').toLowerCase();
}

export function copyAttributes(from: Element, to: Element, filter?: string[]): void {
	for (const { name, value } of from.attributes) {
		if (!filter || filter.includes(name)) {
			to.setAttribute(name, value);
		}
	}
}

export function isEditableElement(element: Element): boolean {
	return element.tagName.toLowerCase() === 'input' || element.tagName.toLowerCase() === 'textarea' || isHTMLElement(element) && !!element.editContext;
}

/**
 * A custom element that fires callbacks when connected to or disconnected from the DOM.
 * Useful for tracking whether a template or component is currently mounted, especially
 * with iframes/webviews that are sensitive to movement.
 *
 * @example
 * ```ts
 * const observer = document.createElement('connection-observer') as ConnectionObserverElement;
 * observer.onDidConnect = () => console.log('mounted');
 * observer.onDidDisconnect = () => console.log('unmounted');
 * container.appendChild(observer);
 * ```
 */
export class ConnectionObserverElement extends HTMLElement {
}

if (!customElements.get('connection-observer')) {
	customElements.define('connection-observer', ConnectionObserverElement);
}
