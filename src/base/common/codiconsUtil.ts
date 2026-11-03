/*---------------------------------------------------------------------------------------------
 *  Copyright (c) Microsoft Corporation. All rights reserved.
 *  Licensed under the MIT License. See LICENSE.vscode.txt in the project root for license information.
 *--------------------------------------------------------------------------------------------*/
import { ThemeIcon } from './themables.js';
import { isString } from './types.js';


const _codiconFontCharacters: { [id: string]: number } = Object.create(null);

export function register(id: string, fontCharacter: number | string): ThemeIcon {
	if (isString(fontCharacter)) {
		const val = _codiconFontCharacters[fontCharacter];
		if (val === undefined) {
			throw new Error(`${id} references an unknown codicon: ${fontCharacter}`);
		}
		fontCharacter = val;
	}
	_codiconFontCharacters[id] = fontCharacter;
	return { id };
}
