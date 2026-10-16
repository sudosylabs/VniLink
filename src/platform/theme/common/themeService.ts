/*---------------------------------------------------------------------------------------------
 *  Copyright (c) Microsoft Corporation. All rights reserved.
 *  Licensed under the MIT License. See LICENSE.vscode.txt in the project root for license information.
 *  Adapted for VniLink: native window controls and window state only.
 *--------------------------------------------------------------------------------------------*/

import { createDecorator } from '../../instantiation/common/instantiation.js';
import type { Event } from '../../../base/common/event.js';

export const IThemeService = createDecorator<IThemeService>('themeService');

export interface ITitlebarTheme {
	readonly background: string;
	readonly foreground: string;
	readonly inactiveForeground: string;
}

export interface IThemeService {
	readonly _serviceBrand: undefined;
	readonly onDidColorThemeChange: Event<ITitlebarTheme>;
	getColorTheme(): ITitlebarTheme;
}
