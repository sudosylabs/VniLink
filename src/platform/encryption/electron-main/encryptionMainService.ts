/*---------------------------------------------------------------------------------------------
 *  Copyright (c) Microsoft Corporation. All rights reserved.
 *  Licensed under the MIT License. See LICENSE.vscode.txt in the project root for license information.
 *  Adapted for VniLink: same provider handling and logging as upstream.
 *--------------------------------------------------------------------------------------------*/

import { safeStorage, app } from 'electron';
import { isMacintosh, isWindows } from '../../../base/common/platform.js';
import { KnownStorageProvider, PasswordStoreCLIOption } from '../common/encryption.js';
import type { IEncryptionMainService } from '../common/encryption.js';
import { ILogService } from '../../log/common/log.js';

// The stock Electron API surface below is required by our pinned Electron
// version, so typecheck fails the build if availability ever changes.
const knownStorageBackends: Record<string, KnownStorageProvider | undefined> = {
	[KnownStorageProvider.basicText]: KnownStorageProvider.basicText,
	[KnownStorageProvider.gnomeAny]: KnownStorageProvider.gnomeAny,
	[KnownStorageProvider.gnomeLibsecret]: KnownStorageProvider.gnomeLibsecret,
	[KnownStorageProvider.gnomeKeyring]: KnownStorageProvider.gnomeKeyring,
	[KnownStorageProvider.kwallet]: KnownStorageProvider.kwallet,
	[KnownStorageProvider.kwallet5]: KnownStorageProvider.kwallet5,
	[KnownStorageProvider.kwallet6]: KnownStorageProvider.kwallet6,
};

export class EncryptionMainService implements IEncryptionMainService {
	declare public readonly _serviceBrand: undefined;

	constructor(
		@ILogService private readonly logService: ILogService
	) {
		// if this commandLine switch is set, the user has opted in to using basic text encryption
		if (app.commandLine.getSwitchValue('password-store') as PasswordStoreCLIOption === PasswordStoreCLIOption.basic) {
			this.logService.trace('[EncryptionMainService] setting usePlainTextEncryption to true...');
			safeStorage.setUsePlainTextEncryption(true);
			this.logService.trace('[EncryptionMainService] set usePlainTextEncryption to true');
		}
	}

	public async encrypt(value: string): Promise<string> {
		this.logService.trace('[EncryptionMainService] Encrypting value...');
		try {
			const result = JSON.stringify(safeStorage.encryptString(value));
			this.logService.trace('[EncryptionMainService] Encrypted value.');
			return result;
		} catch (e) {
			this.logService.error(e instanceof Error ? e : new Error(String(e)));
			throw e;
		}
	}

	public async decrypt(value: string): Promise<string> {
		let parsedValue: unknown;
		try {
			parsedValue = JSON.parse(value);
		} catch {
			throw new Error('[EncryptionMainService] Invalid encrypted value');
		}
		if (!record(parsedValue) || !parsedValue.data) {
			throw new Error('[EncryptionMainService] Invalid encrypted value');
		}
		const data: unknown = parsedValue.data;
		let bufferToDecrypt: Buffer;
		if (typeof data === 'string') {
			bufferToDecrypt = Buffer.from(data);
		} else if (Array.isArray(data) && data.every((entry: unknown): entry is number => typeof entry === 'number')) {
			bufferToDecrypt = Buffer.from(data);
		} else {
			throw new Error('[EncryptionMainService] Invalid encrypted value');
		}
		this.logService.trace('[EncryptionMainService] Decrypting value...');
		try {
			const result = safeStorage.decryptString(bufferToDecrypt);
			this.logService.trace('[EncryptionMainService] Decrypted value.');
			return result;
		} catch (e) {
			this.logService.error(e instanceof Error ? e : new Error(String(e)));
			throw e;
		}
	}

	public isEncryptionAvailable(): Promise<boolean> {
		this.logService.trace('[EncryptionMainService] Checking if encryption is available...');
		const result = safeStorage.isEncryptionAvailable();
		this.logService.trace('[EncryptionMainService] Encryption is available: ', result);
		return Promise.resolve(result);
	}

	public getKeyStorageProvider(): Promise<KnownStorageProvider> {
		if (isWindows) {
			return Promise.resolve(KnownStorageProvider.dplib);
		}
		if (isMacintosh) {
			return Promise.resolve(KnownStorageProvider.keychainAccess);
		}
		try {
			this.logService.trace('[EncryptionMainService] Getting selected storage backend...');
			const backend = safeStorage.getSelectedStorageBackend();
			const result = knownStorageBackends[backend] ?? KnownStorageProvider.unknown;
			this.logService.trace('[EncryptionMainService] Selected storage backend: ', result);
			return Promise.resolve(result);
		} catch (e) {
			this.logService.error(e instanceof Error ? e : new Error(String(e)));
		}
		return Promise.resolve(KnownStorageProvider.unknown);
	}

	public async setUsePlainTextEncryption(): Promise<void> {
		if (isWindows) {
			throw new Error('Setting plain text encryption is not supported on Windows.');
		}
		if (isMacintosh) {
			throw new Error('Setting plain text encryption is not supported on macOS.');
		}
		this.logService.trace('[EncryptionMainService] Setting usePlainTextEncryption to true...');
		safeStorage.setUsePlainTextEncryption(true);
		this.logService.trace('[EncryptionMainService] Set usePlainTextEncryption to true');
	}
}

function record(value: unknown): value is Record<string, unknown> {
	return typeof value === 'object' && value !== null && !Array.isArray(value);
}
