/*---------------------------------------------------------------------------------------------
 *  Copyright (c) Microsoft Corporation. All rights reserved.
 *  Licensed under the MIT License. See LICENSE.vscode.txt in the project root for license information.
 *  Adapted for VniLink: native window controls and window state only.
 *--------------------------------------------------------------------------------------------*/

import { isTahoeOrNewer } from '../../../base/common/platform.js';
export interface IPoint {
	readonly x: number;
	readonly y: number;
}
export function getMacOSWindowControlsPosition(
	height: number,
	osVersion: string,
	horizontalInset?: number,
): IPoint | null {
	const buttonHeight = isTahoeOrNewer(osVersion) ? 14 : 16;
	const offset = Math.floor((height - buttonHeight) / 2);
	return !offset && horizontalInset === undefined ? null : { x: horizontalInset ?? offset + 1, y: offset };
}
export function getNativeTitlebarHeight(platform: string, osVersion: string): number {
	return platform === 'darwin' ? (isTahoeOrNewer(osVersion) ? 32 : 28) : 30;
}
