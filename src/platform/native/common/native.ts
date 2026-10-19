/*---------------------------------------------------------------------------------------------
 *  Copyright (c) Microsoft Corporation. All rights reserved.
 *  Licensed under the MIT License. See LICENSE.vscode.txt in the project root for license information.
 *  Adapted for VniLink: native window controls, window state, folder picking, message boxes, displays, allowlisted links, clipboard and relaunch.
 *--------------------------------------------------------------------------------------------*/

import type { WindowBackgroundMaterial } from '../../window/common/windowMaterial.js';
import type { NativeScreenSource } from '../../engine/common/engine.js';
import type { Event } from '../../../base/common/event.js';
import { VSBuffer } from '../../../base/common/buffer.js';
import type { MessageBoxOptions, MessageBoxReturnValue } from '../../../base/parts/sandbox/common/electronTypes.js';
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
	pickFolder(defaultPath: string): Promise<string | null>;
	/** A native message box over the window, as VS Code's desktop dialogs are. */
	showMessageBox(options: MessageBoxOptions & ITargetWindowOptions): Promise<MessageBoxReturnValue>;
	getCaptureDisplays(): Promise<NativeScreenSource[]>;
	openExternal(url: string): Promise<void>;

	// Clipboard
	readClipboardText(type?: 'selection' | 'clipboard'): Promise<string>;
	writeClipboardText(text: string, type?: 'selection' | 'clipboard'): Promise<void>;
	/** Places a PNG picture on the clipboard. */
	writeClipboardPicture(png: VSBuffer): Promise<void>;

	/** Restarts the application; main refuses unless the engine requires a restart. */
	relaunch(): Promise<void>;
}

/** Main-process IPC channel serving {@link INativeHostService}. */
export const NATIVE_HOST_CHANNEL_NAME = 'nativeHost';

export type NativeHostMethod =
	| 'getWindowState'
	| 'focusWindow'
	| 'minimizeWindow'
	| 'maximizeWindow'
	| 'unmaximizeWindow'
	| 'closeWindow'
	| 'setFullScreen'
	| 'updateWindowControls'
	| 'pickFolder'
	| 'showMessageBox'
	| 'getCaptureDisplays'
	| 'openExternal'
	| 'readClipboardText'
	| 'writeClipboardText'
	| 'writeClipboardPicture'
	| 'relaunch';

/** Longest text a window may place on the clipboard. */
const MAX_CLIPBOARD_TEXT = 16 * 1024 * 1024;

/** Largest picture a window may save. */
const MAX_PICTURE_BYTES = 32 * 1024 * 1024;

const pngSignature = [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a];

export function isPng(value: unknown): value is VSBuffer {
	return value instanceof VSBuffer && value.byteLength > pngSignature.length && value.byteLength <= MAX_PICTURE_BYTES
		&& pngSignature.every((byte, index) => value.buffer[index] === byte);
}

function isClipboardType(value: unknown): boolean {
	return value === undefined || value === 'selection' || value === 'clipboard';
}

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
		(value.backgroundMaterial === 'none' || value.backgroundMaterial === 'vibrancy' || value.backgroundMaterial === 'glass' || value.backgroundMaterial === 'mica')
	);
}

const messageBoxKeys = ['message', 'detail', 'type', 'buttons', 'defaultId', 'cancelId', 'title', 'noLink', 'targetWindowId'];

/** A short message box with at most four buttons; nothing that reaches files or links. */
export function isMessageBoxOptions(value: unknown, windowId: number): value is MessageBoxOptions & ITargetWindowOptions {
	if (typeof value !== 'object' || value === null || Array.isArray(value) || Object.keys(value).some(key => !messageBoxKeys.includes(key))) {
		return false;
	}
	const shortText = (field: unknown, max: number): boolean => field === undefined || (typeof field === 'string' && field.length <= max && !field.includes('\0'));
	const buttons: unknown = Reflect.get(value, 'buttons');
	const count = Array.isArray(buttons) ? buttons.length : 0;
	const index = (field: unknown): boolean => field === undefined || (Number.isInteger(field) && Number(field) >= 0 && Number(field) < Math.max(count, 1));
	const type: unknown = Reflect.get(value, 'type');
	const target: unknown = Reflect.get(value, 'targetWindowId');
	return typeof Reflect.get(value, 'message') === 'string' && shortText(Reflect.get(value, 'message'), 500)
		&& shortText(Reflect.get(value, 'detail'), 1000) && shortText(Reflect.get(value, 'title'), 100)
		&& (type === undefined || type === 'none' || type === 'info' || type === 'warning' || type === 'question')
		&& (buttons === undefined || (Array.isArray(buttons) && buttons.length <= 4 && buttons.every(button => typeof button === 'string' && button.length > 0 && button.length <= 64)))
		&& index(Reflect.get(value, 'defaultId')) && index(Reflect.get(value, 'cancelId'))
		&& (Reflect.get(value, 'noLink') === undefined || typeof Reflect.get(value, 'noLink') === 'boolean')
		&& (target === undefined || target === windowId);
}

export function isNativeHostRequest(method: unknown, args: unknown): method is NativeHostMethod {
	if (typeof method !== 'string' || !Array.isArray(args) || !Number.isSafeInteger(args[0]) || args[0] <= 0) {
		return false;
	}
	if (method === 'getWindowState' || method === 'getCaptureDisplays' || method === 'relaunch') {
		return args.length === 1;
	}
	if (method === 'readClipboardText') {
		return args.length <= 2 && isClipboardType(args[1]);
	}
	if (method === 'writeClipboardText') {
		return args.length >= 2 && args.length <= 3 && typeof args[1] === 'string' && args[1].length <= MAX_CLIPBOARD_TEXT && isClipboardType(args[2]);
	}
	if (method === 'writeClipboardPicture') {
		return args.length === 2 && isPng(args[1]);
	}
	if (method === 'showMessageBox') {
		return args.length === 2 && isMessageBoxOptions(args[1], Number(args[0]));
	}
	if (method === 'pickFolder' || method === 'openExternal') {
		return args.length === 2 && typeof args[1] === 'string' && args[1].length > 0 && args[1].length <= 4096 && !args[1].includes('\0');
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
