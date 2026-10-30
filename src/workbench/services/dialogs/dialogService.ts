// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import { createDecorator } from '../../../platform/instantiation/common/instantiation.js';

export const IDialogService = createDecorator<IDialogService>('dialogService');

/** A question with a few answers, in the role of VS Code's `IDialogService`. */
export interface IDialogOptions {
	readonly type: 'warning' | 'info' | 'error';
	readonly message: string;
	readonly detail?: string;
	/** The answers, in order; `cancelId` names the one that dismisses. */
	readonly buttons: readonly string[];
	readonly cancelId: number;
	/** A checkbox under the message, such as "Do the same for the other items". */
	readonly checkbox?: {
		readonly label: string;
		readonly checked?: boolean;
	};
}

export interface IDialogResult {
	/** The index of the chosen button. */
	readonly button: number;
	/** Whether the checkbox was ticked when the dialog closed; `false` without one. */
	readonly checked: boolean;
}

/**
 * Asks the person at this computer. macOS shows its native box, which
 * follows the system's appearance; elsewhere VniLink draws VS Code's dialog
 * in the app's theme, since a native Windows box is always light.
 */
export interface IDialogService {
	readonly _serviceBrand: undefined;
	show(options: IDialogOptions): Promise<IDialogResult>;
}
