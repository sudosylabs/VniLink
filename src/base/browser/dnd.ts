/*---------------------------------------------------------------------------------------------
 *  Copyright (c) Microsoft Corporation. All rights reserved.
 *  Licensed under the MIT License. See LICENSE.vscode.txt in the project root for license information.
 *--------------------------------------------------------------------------------------------*/

import { Mimes } from '../common/mime.js';

// Common data transfers
export const DataTransfers = {

	/**
	 * Browser specific transfer type for files
	 */
	FILES: 'Files',

	/**
	 * Typically transfer type for copy/paste transfers.
	 */
	TEXT: Mimes.text,
};

export interface IDragAndDropData {
	update(dataTransfer: DataTransfer): void;
	getData(): unknown;
}
