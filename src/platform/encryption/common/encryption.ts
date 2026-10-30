/*---------------------------------------------------------------------------------------------
 *  Copyright (c) Microsoft Corporation. All rights reserved.
 *  Licensed under the MIT License. See LICENSE.vscode.txt in the project root for license information.
 *  Adapted from VS Code 1.141.0 src/vs/platform/encryption/common/encryptionService.ts.
 *--------------------------------------------------------------------------------------------*/

import { createDecorator } from '../../instantiation/common/instantiation.js';

export const IEncryptionMainService = createDecorator<IEncryptionMainService>('encryptionMainService');

export interface ICommonEncryptionService {
	readonly _serviceBrand: undefined;
	encrypt(value: string): Promise<string>;
	decrypt(value: string): Promise<string>;
	isEncryptionAvailable(): Promise<boolean>;
}

export interface IEncryptionMainService extends ICommonEncryptionService {
	setUsePlainTextEncryption(): Promise<void>;
	getKeyStorageProvider(): Promise<KnownStorageProvider>;
}
// The values provided to the `password-store` command line switch.
// Notice that they are not the same as the values returned by
// `getSelectedStorageBackend` in the `safeStorage` API.
export const enum PasswordStoreCLIOption {
	kwallet = 'kwallet',
	kwallet5 = 'kwallet5',
	gnomeLibsecret = 'gnome-libsecret',
	basic = 'basic'
}

export const enum KnownStorageProvider {
	unknown = 'unknown',
	basicText = 'basic_text',
	gnomeAny = 'gnome_any',
	gnomeLibsecret = 'gnome_libsecret',
	gnomeKeyring = 'gnome_keyring',
	kwallet = 'kwallet',
	kwallet5 = 'kwallet5',
	kwallet6 = 'kwallet6',
	dplib = 'dpapi',
	keychainAccess = 'keychain_access',
}

export function isEncryptionText(value: unknown): value is string {
	return typeof value === 'string' && value.length > 0 && value.length <= 64 * 1024;
}
