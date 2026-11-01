/*---------------------------------------------------------------------------------------------
 *  Copyright (c) Microsoft Corporation. All rights reserved.
 *  Licensed under the MIT License. See LICENSE.vscode.txt in the project root for license information.
 *--------------------------------------------------------------------------------------------*/

import { isMacintosh, isWindows } from './platform.js';

/**
 * Handles mnemonics for buttons. Depending on OS:
 * - Windows: Supported via & character (replace && with & and & with && for escaping)
 * -   Linux: Supported via _ character (replace && with _)
 * -   macOS: Unsupported (replace && with empty string)
 * When forceDisableMnemonics is set, returns just the label without mnemonics.
 */
export function mnemonicButtonLabel(label: string, forceDisableMnemonics: true): string;
export function mnemonicButtonLabel(label: string, forceDisableMnemonics?: false): { readonly withMnemonic: string; readonly withoutMnemonic: string };
export function mnemonicButtonLabel(label: string, forceDisableMnemonics?: boolean): { readonly withMnemonic: string; readonly withoutMnemonic: string } | string {
	const withoutMnemonic = label.replace(/\(&&\w\)|&&/g, '');

	if (forceDisableMnemonics) {
		return withoutMnemonic;
	}
	if (isMacintosh) {
		return { withMnemonic: withoutMnemonic, withoutMnemonic };
	}

	let withMnemonic: string;
	if (isWindows) {
		withMnemonic = label.replace(/&&|&/g, m => m === '&' ? '&&' : '&');
	} else {
		withMnemonic = label.replace(/&&/g, '_');
	}
	return { withMnemonic, withoutMnemonic };
}

export function unmnemonicLabel(label: string): string {
	return label.replace(/&/g, '&&');
}
