/*---------------------------------------------------------------------------------------------
 *  Copyright (c) Microsoft Corporation. All rights reserved.
 *  Licensed under the MIT License. See LICENSE.vscode.txt in the project root for license information.
 *--------------------------------------------------------------------------------------------*/

import { IDimension } from '../../../base/browser/dom.js';
import { Event } from '../../../base/common/event.js';
import { createDecorator } from '../../instantiation/common/instantiation.js';

export const ILayoutService = createDecorator<ILayoutService>('layoutService');

export interface ILayoutService {

	readonly _serviceBrand: undefined;

	/**
	 * An event that is emitted when the main container is layed out.
	 */
	readonly onDidLayoutMainContainer: Event<IDimension>;

	/**
	 * An event that is emitted when any container is layed out.
	 */
	readonly onDidLayoutContainer: Event<{ readonly container: HTMLElement; readonly dimension: IDimension }>;

	/**
	 * Main container of the application.
	 */
	readonly mainContainer: HTMLElement;

	/**
	 * Active container of the application. When multiple windows are opened, will return
	 * the container of the active, focused window.
	 */
	readonly activeContainer: HTMLElement;

	/**
	 * Get the container for the given window.
	 */
	getContainer(window: Window): HTMLElement;

	/**
	 * Focus the primary component of the active container.
	 */
	focus(): void;
}
