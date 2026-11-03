/*---------------------------------------------------------------------------------------------
 *  Copyright (c) Microsoft Corporation. All rights reserved.
 *  Licensed under the MIT License. See LICENSE.vscode.txt in the project root for license information.
 *--------------------------------------------------------------------------------------------*/
import { register } from './codiconsUtil.js';
import { codiconsLibrary } from './codiconsLibrary.js';


/**
 * Derived icons, that could become separate icons.
 * These mappings should be moved into the mapping file in the vscode-codicons repo at some point.
 */
export const codiconsDerived = {
	dialogError: register('dialog-error', 'error'),
	dialogWarning: register('dialog-warning', 'warning'),
	dialogInfo: register('dialog-info', 'info'),
	dialogClose: register('dialog-close', 'close'),
	scrollbarButtonLeft: register('scrollbar-button-left', 'triangle-left'),
	scrollbarButtonRight: register('scrollbar-button-right', 'triangle-right'),
	scrollbarButtonUp: register('scrollbar-button-up', 'triangle-up'),
	scrollbarButtonDown: register('scrollbar-button-down', 'triangle-down'),
	dropDownButton: register('drop-down-button', 0xeab4),

} as const;

/**
 * The Codicon library is a set of default icons that are built-in in VS Code.
 *
 * In the product (outside of base) Codicons should only be used as defaults. In order to have all icons in VS Code
 * themeable, component should define new, UI component specific icons using `iconRegistry.registerIcon`.
 * In that call a Codicon can be named as default.
 */
export const Codicon = {
	...codiconsLibrary,
	...codiconsDerived

} as const;
