/*---------------------------------------------------------------------------------------------
 *  Copyright (c) Microsoft Corporation. All rights reserved.
 *  Licensed under the MIT License. See LICENSE.vscode.txt in the project root for license information.
 *  Adapted for VniLink: native window controls and window state only.
 *--------------------------------------------------------------------------------------------*/

import type { WindowBackgroundMaterial } from '../../window/common/windowMaterial.js';
import type { Event } from '../../../base/common/event.js';
import { createDecorator } from '../../instantiation/common/instantiation.js';

export interface IWindowState {
	readonly windowId: number;
	readonly revision: number;
	readonly focused: boolean;
	readonly maximized: boolean;
	readonly fullscreen: boolean;
	readonly alwaysOnTop: boolean;
	readonly zoomFactor: number;
	readonly backgroundMaterial: WindowBackgroundMaterial;
}

export interface IWindowControlsOptions {
	readonly targetWindowId?: number;
	readonly height?: number;
	readonly horizontalInset?: number;
	readonly backgroundColor?: string;
	readonly foregroundColor?: string;
}

export interface ITargetWindowOptions {
	readonly targetWindowId?: number;
}

export const INativeHostService = createDecorator<INativeHostService>('nativeHostService');

export interface INativeHostService {
	readonly _serviceBrand: undefined;
	readonly windowId: number;
	readonly onDidChangeWindowState: Event<IWindowState>;
	getWindowState(): Promise<IWindowState>;
	focusWindow(options?: ITargetWindowOptions): Promise<void>;
	minimizeWindow(options?: ITargetWindowOptions): Promise<void>;
	maximizeWindow(options?: ITargetWindowOptions): Promise<void>;
	unmaximizeWindow(options?: ITargetWindowOptions): Promise<void>;
	closeWindow(options?: ITargetWindowOptions): Promise<void>;
	setFullScreen(fullscreen: boolean, options?: ITargetWindowOptions): Promise<void>;
	updateWindowControls(options: IWindowControlsOptions): Promise<void>;
}

export type NativeHostMethod =
	| 'getWindowState'
	| 'focusWindow'
	| 'minimizeWindow'
	| 'maximizeWindow'
	| 'unmaximizeWindow'
	| 'closeWindow'
	| 'setFullScreen'
	| 'updateWindowControls';

export function isWindowState(value: unknown): value is IWindowState {
	if (typeof value !== 'object' || value === null) {
		return false;
	}
	return (
		'windowId' in value &&
		Number.isSafeInteger(value.windowId) &&
		Number(value.windowId) > 0 &&
		'revision' in value &&
		Number.isSafeInteger(value.revision) &&
		Number(value.revision) >= 0 &&
		'focused' in value &&
		typeof value.focused === 'boolean' &&
		'maximized' in value &&
		typeof value.maximized === 'boolean' &&
		'fullscreen' in value &&
		typeof value.fullscreen === 'boolean' &&
		'alwaysOnTop' in value &&
		typeof value.alwaysOnTop === 'boolean' &&
		'zoomFactor' in value &&
		typeof value.zoomFactor === 'number' &&
		Number.isFinite(value.zoomFactor) &&
		value.zoomFactor >= 0.25 &&
		value.zoomFactor <= 5 &&
		'backgroundMaterial' in value &&
		(value.backgroundMaterial === 'none' || value.backgroundMaterial === 'vibrancy' || value.backgroundMaterial === 'mica')
	);
}

export function isNativeHostRequest(method: unknown, args: unknown): method is NativeHostMethod {
	if (typeof method !== 'string' || !Array.isArray(args) || !Number.isSafeInteger(args[0]) || args[0] <= 0) {
		return false;
	}
	if (method === 'getWindowState') {
		return args.length === 1;
	}
	let options: unknown;
	if (method === 'setFullScreen') {
		if (typeof args[1] !== 'boolean' || args.length > 3) {
			return false;
		}
		options = args[2];
	} else {
		if (
			![
				'focusWindow',
				'minimizeWindow',
				'maximizeWindow',
				'unmaximizeWindow',
				'closeWindow',
				'updateWindowControls',
			].includes(method) ||
			args.length > 2
		) {
			return false;
		}
		options = args[1];
	}
	if (options === undefined) {
		return method !== 'updateWindowControls';
	}
	if (typeof options !== 'object' || options === null || Array.isArray(options)) {
		return false;
	}
	const allowed =
		method === 'updateWindowControls'
			? ['targetWindowId', 'height', 'horizontalInset', 'backgroundColor', 'foregroundColor']
			: ['targetWindowId'];
	if (Object.keys(options).some((key) => !allowed.includes(key))) {
		return false;
	}
	if ('targetWindowId' in options && options.targetWindowId !== args[0]) {
		return false;
	}
	if (
		'height' in options &&
		(typeof options.height !== 'number' ||
			!Number.isInteger(options.height) ||
			options.height < 1 ||
			options.height > 200)
	) {
		return false;
	}
	if (
		'horizontalInset' in options &&
		(typeof options.horizontalInset !== 'number' ||
			!Number.isInteger(options.horizontalInset) ||
			options.horizontalInset < 0 ||
			options.horizontalInset > 200)
	) {
		return false;
	}
	for (const key of ['backgroundColor', 'foregroundColor'] as const) {
		if (
			Object.hasOwn(options, key) &&
			(typeof Reflect.get(options, key) !== 'string' ||
				!/^#[0-9a-f]{6}$/i.test(String(Reflect.get(options, key))))
		) {
			return false;
		}
	}
	return true;
}
