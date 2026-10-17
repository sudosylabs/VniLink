import { permissionKinds, type MachinePermission, type PermissionCommand, type PermissionKind, type PermissionSnapshot, type PermissionStatus } from '../common/permissions.js';

type Preferences = Pick<Electron.SystemPreferences, 'getMediaAccessStatus' | 'askForMediaAccess' | 'isTrustedAccessibilityClient'>;

const macSettings: Record<PermissionKind, string> = {
	screen: 'Privacy_ScreenCapture', accessibility: 'Privacy_Accessibility', camera: 'Privacy_Camera', microphone: 'Privacy_Microphone',
};
/** OS consent is queried live; it is never stored as an application preference. */
export class MachinePermissions {
	private requesting = false;

	constructor(private readonly platform: string, private readonly development: boolean, private readonly preferences: Preferences, private readonly openExternal: (url: string) => Promise<void>) { }

	private permission(kind: PermissionKind): MachinePermission {
		let status: PermissionStatus = 'unsupported';
		if (this.platform === 'darwin' || (this.platform === 'win32' && (kind === 'camera' || kind === 'microphone'))) {
			try {
				status = kind === 'accessibility' ? (this.preferences.isTrustedAccessibilityClient(false) ? 'granted' : 'denied') : this.preferences.getMediaAccessStatus(kind);
			} catch {
				status = 'unknown';
			}
		}
		return { kind, status, canRequest: this.platform === 'darwin' && (kind === 'accessibility' ? status === 'denied' : (kind === 'camera' || kind === 'microphone') && status === 'not-determined'), canOpenSettings: this.platform === 'darwin' || (this.platform === 'win32' && (kind === 'camera' || kind === 'microphone')) };
	}

	public snapshot(): PermissionSnapshot {
		return { development: this.development, permissions: permissionKinds.map(kind => this.permission(kind)) };
	}

	public async execute(command: PermissionCommand): Promise<PermissionSnapshot> {
		if (command.type === 'status') {
			return this.snapshot();
		}
		const permission = this.permission(command.kind);
		if (command.type === 'request') {
			if (!permission.canRequest || this.requesting) {
				throw new Error('Permission request unavailable');
			}
			this.requesting = true;
			try {
				if (command.kind === 'accessibility') {
					this.preferences.isTrustedAccessibilityClient(true);
				} else if (command.kind === 'camera' || command.kind === 'microphone') {
					await this.preferences.askForMediaAccess(command.kind);
				}
			} finally {
				this.requesting = false;
			}
		} else {
			if (!permission.canOpenSettings) {
				throw new Error('Permission settings unavailable');
			}
			// Only fixed OS settings destinations can cross this boundary.
			await this.openExternal(this.platform === 'darwin' ? `x-apple.systempreferences:com.apple.preference.security?${macSettings[command.kind]}` : command.kind === 'camera' ? 'ms-settings:privacy-webcam' : 'ms-settings:privacy-microphone');
		}
		return this.snapshot();
	}
}
