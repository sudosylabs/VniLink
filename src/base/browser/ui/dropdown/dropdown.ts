/*---------------------------------------------------------------------------------------------
 *  Copyright (c) Microsoft Corporation. All rights reserved.
 *  Licensed under the MIT License. See LICENSE.vscode.txt in the project root for license information.
 *--------------------------------------------------------------------------------------------*/

import { IAction } from '../../../common/actions.js';
import './dropdown.css';

export interface IActionProvider {
	getActions(): readonly IAction[];
}

export function isActionProvider(obj: unknown): obj is IActionProvider {
	const candidate = obj as IActionProvider | undefined;

	return typeof candidate?.getActions === 'function';
}
