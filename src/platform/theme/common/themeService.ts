/*---------------------------------------------------------------------------------------------
 *  Copyright (c) Microsoft Corporation. All rights reserved.
 *  Licensed under the MIT License. See LICENSE.vscode.txt in the project root for license information.
 *  Adapted for VniLink: native window controls and window state only.
 *--------------------------------------------------------------------------------------------*/
// ---------------------------------------------------------------------------------------------
// Modifications Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Modifications licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only AND MIT
// ---------------------------------------------------------------------------------------------

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
