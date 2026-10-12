import { app, safeStorage } from 'electron';
import { isMacintosh, isWindows } from '../../../base/common/platform.js';
import type { IEncryptionMainService } from '../common/encryption.js';
import { KnownStorageProvider } from '../common/encryption.js';
// setUsePlainTextEncryption and getSelectedStorageBackend are stock
// Electron safeStorage APIs; the Linux-only paths stay guarded by callers.
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
	constructor() {
		if (app.commandLine.getSwitchValue('password-store') === 'basic') {
			safeStorage.setUsePlainTextEncryption(true);
		}
	}
	public encrypt(value: string): Promise<string> {
		return Promise.resolve(JSON.stringify(safeStorage.encryptString(value)));
	}
	public decrypt(value: string): Promise<string> {
		let parsed: unknown;
		try {
			parsed = JSON.parse(value);
		} catch {
			throw new Error('Invalid encrypted value');
		}
		if (typeof parsed !== 'object' || parsed === null || !('data' in parsed)) {
			throw new Error('Invalid encrypted value');
		}
		return Promise.resolve(safeStorage.decryptString(Buffer.from((parsed as { data: string }).data)));
	}
	public isEncryptionAvailable(): Promise<boolean> {
		return Promise.resolve(safeStorage.isEncryptionAvailable());
	}
	public getKeyStorageProvider(): Promise<KnownStorageProvider> {
		if (isWindows) {
			return Promise.resolve(KnownStorageProvider.dplib);
		}
		if (isMacintosh) {
			return Promise.resolve(KnownStorageProvider.keychainAccess);
		}
		try {
			const backend = safeStorage.getSelectedStorageBackend();
			if (typeof backend === 'string') {
				return Promise.resolve(knownStorageBackends[backend] ?? KnownStorageProvider.unknown);
			}
		} catch {
			// Fall through to unknown below.
		}
		return Promise.resolve(KnownStorageProvider.unknown);
	}
	public setUsePlainTextEncryption(): Promise<void> {
		if (isWindows) {
			return Promise.reject(new Error('Setting plain text encryption is not supported on Windows.'));
		}
		if (isMacintosh) {
			return Promise.reject(new Error('Setting plain text encryption is not supported on macOS.'));
		}
		safeStorage.setUsePlainTextEncryption(true);
		return Promise.resolve();
	}
}
