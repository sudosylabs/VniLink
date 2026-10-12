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
