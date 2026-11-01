/*---------------------------------------------------------------------------------------------
 *  Copyright (c) Microsoft Corporation. All rights reserved.
 *  Licensed under the MIT License. See LICENSE.vscode.txt in the project root for license information.
 *--------------------------------------------------------------------------------------------*/

import { URI } from '../../../base/common/uri.js';
import { ContextKeyExpression } from '../../contextkey/common/contextkey.js';
import { ThemeIcon } from '../../../base/common/themables.js';
import { ICommandMetadata } from '../../commands/common/commands.js';

export interface ILocalizedString {

	/**
	 * The localized value of the string.
	 */
	value: string;

	/**
	 * The original (non localized value of the string)
	 */
	original: string;
}

export interface ICommandActionTitle extends ILocalizedString {
}

export type Icon = { dark?: URI; light?: URI } | ThemeIcon;

export interface ICommandActionToggleInfo {
}

export interface ICommandAction {
	id: string;
	title: string | ICommandActionTitle;
	shortTitle?: string | ICommandActionTitle;
	/**
	 * Metadata about this command, used for:
	 * - API commands
	 * - when showing keybindings that have no other UX
	 * - when searching for commands in the Command Palette
	 */
	metadata?: ICommandMetadata;
	tooltip?: string | ILocalizedString;
	icon?: Icon;
	/**
	 * Precondition controls enablement (for example for a menu item, show
	 * it in grey or for a command, do not allow to invoke it)
	 */
	precondition?: ContextKeyExpression;

	/**
	 * The action is a toggle action. Define the context key expression that reflects its toggle-state
	 * or define toggle-info including an icon and a title that goes well with a checkmark.
	 */
	toggled?: ContextKeyExpression | ICommandActionToggleInfo;
}
