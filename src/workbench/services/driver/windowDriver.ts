/*---------------------------------------------------------------------------------------------
 *  Copyright (c) Microsoft Corporation. All rights reserved.
 *  Licensed under the MIT License. See LICENSE.vscode.txt in the project root for license information.
 *  Adapted from VS Code 1.141.0 src/vs/workbench/services/driver/common/driver.ts.
 *--------------------------------------------------------------------------------------------*/
// ---------------------------------------------------------------------------------------------
// Modifications Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Modifications licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only AND MIT
// ---------------------------------------------------------------------------------------------

import type { EngineCommand, EngineReply } from '../../../platform/engine/common/engine.js';

export interface IElement {
	readonly tagName: string;
	readonly className: string;
	readonly textContent: string;
	readonly attributes: Readonly<Record<string, string>>;
	readonly children: IElement[];
	readonly top: number;
	readonly left: number;
}

/** Where an element is drawn, how far it scrolls, and the computed styles asked for. */
export interface IElementLayout {
	readonly top: number;
	readonly left: number;
	readonly right: number;
	readonly bottom: number;
	readonly width: number;
	readonly height: number;
	readonly scrollTop: number;
	readonly scrollWidth: number;
	readonly clientWidth: number;
	readonly scrollHeight: number;
	readonly clientHeight: number;
	readonly hidden: boolean;
	readonly draggable: boolean;
	readonly style: Readonly<Record<string, string>>;
}

/** What a page's script can reach, which an isolated renderer keeps from it. */
export interface IRendererExposure {
	readonly configuration: unknown;
	readonly nodeExposed: boolean;
	readonly vscodeExposed: boolean;
	readonly unprefixedRejected: boolean;
	readonly api: readonly string[];
	readonly windowOpened: boolean;
}

/** What a window heard while its engine restarted, in order, and whether it still thinks it is restarting. */
export interface IEngineRestartStory {
	readonly story: readonly string[];
	readonly restarting: boolean;
}

export interface IWindowDriver {
	setValue(selector: string, text: string): Promise<void>;
	isActiveElement(selector: string): Promise<boolean>;
	getElements(selector: string, recursive: boolean): Promise<IElement[]>;
	whenWorkbenchRestored(): Promise<void>;

	/** The layout of the first element `selector` matches, with the computed `styles`. */
	getLayout(selector: string, styles?: readonly string[]): Promise<IElementLayout>;
	/** The layout of every element `selector` matches, in document order. */
	getLayouts(selector: string, styles?: readonly string[]): Promise<IElementLayout[]>;
	/** Scrolls the first element `selector` matches to `top` and says where it came to rest. */
	scrollTo(selector: string, top: number): Promise<number>;
	click(selector: string, index?: number): Promise<void>;
	focus(selector: string, index?: number): Promise<void>;
	/** Sends `key` down and up to the `index`th element `selector` matches, or to the window without one. */
	sendKey(key: string, keyCode: number, selector?: string, index?: number): Promise<void>;
	/** Types `text` into the element `selector` matches, as a keyboard's `beforeinput` does. */
	insertText(selector: string, text: string, index?: number): Promise<void>;
	/** A pointer pressed on the page, outside any popover. */
	pointerDownOutside(): Promise<void>;
	/** Resolves once every animation under `selector` that ends has finished and the next frame is drawn. */
	whenAnimationsFinished(selector: string): Promise<void>;
	/** Resolves once two frames have been drawn, so a layout change has painted. */
	whenRendered(): Promise<void>;
	/** Marks the first element `selector` matches, so a later query can say whether it is the same one. */
	mark(selector: string, name: string): Promise<void>;

	rendererExposure(): Promise<IRendererExposure>;
	execute(command: EngineCommand): Promise<EngineReply>;
	updateSetting(key: string, value: unknown): Promise<void>;
	callMainProcess(channel: string, command: string, arg?: unknown): Promise<unknown>;
	permissionStatus(): Promise<unknown>;
	/** The id main knows this window by. */
	getWindowId(): Promise<number>;
	/** Starts recording what the window hears about the engine restarting. */
	listenForEngineRestart(): void;
	/** Waits for the engine to be back after {@link listenForEngineRestart} and says what the window heard. */
	engineRestarted(): Promise<IEngineRestartStory>;
	/** Resolves when the engine reports hosting turned `hosting`; fails after five seconds. */
	whenHosting(hosting: boolean): Promise<void>;
	/** Starts listening for main to report the window always on top. */
	listenForAlwaysOnTop(): void;
	/** The window state main reported after {@link listenForAlwaysOnTop}; fails after five seconds. */
	alwaysOnTop(): Promise<unknown>;
}
