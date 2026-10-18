import { app } from 'electron';
import { mkdir, rename, rm, stat, writeFile } from 'node:fs/promises';
import { homedir } from 'node:os';
import { dirname, join } from 'node:path';
import { Emitter } from '../../../base/common/event.js';
import { Disposable } from '../../../base/common/lifecycle.js';
import { isLinux, isMacintosh } from '../../../base/common/platform.js';
import { isBuilt } from '../../environment/electron-main/environment.js';
import { product } from '../../product/common/product.js';

/** Passed by login items on Windows and Linux; macOS reports a login launch itself. */
export const HIDDEN_ARGUMENT = '--hidden';

/** Whether this platform can keep VniLink running without a window. Windows and Linux need a tray icon to quit from. */
export const canRunInBackground = isMacintosh;

/** The operating system's record of whether VniLink starts when the user signs in. */
export interface LoginItem {
	/** Whether this launch can register itself; development launches would register the development Electron. */
	readonly supported: boolean;
	read(): Promise<boolean>;
	write(enabled: boolean): Promise<void>;
}

/** macOS and Windows keep login items in the OS; Electron reads and writes them. */
class ElectronLoginItem implements LoginItem {
	public readonly supported = isBuilt();

	public read(): Promise<boolean> {
		return Promise.resolve(app.getLoginItemSettings({ args: [HIDDEN_ARGUMENT] }).openAtLogin);
	}

	public write(enabled: boolean): Promise<void> {
		app.setLoginItemSettings({ openAtLogin: enabled, args: [HIDDEN_ARGUMENT] });
		return Promise.resolve();
	}
}

/** Linux desktops start the XDG autostart entries in `~/.config/autostart`. */
export class AutostartLoginItem implements LoginItem {
	public readonly supported = isBuilt();

	constructor(
		private readonly path = join(configHome(), 'autostart', `${product.appId}.desktop`),
		// An AppImage runs from a temporary mount; its own path is what restarts it.
		private readonly executable = process.env.APPIMAGE ?? process.execPath,
	) { }

	public async read(): Promise<boolean> {
		try {
			return (await stat(this.path)).isFile();
		} catch {
			return false;
		}
	}

	public async write(enabled: boolean): Promise<void> {
		if (!enabled) {
			await rm(this.path, { force: true });
			return;
		}
		const entry = autostartEntry(this.executable);
		await mkdir(dirname(this.path), { recursive: true });
		const pending = `${this.path}.tmp`;
		await writeFile(pending, entry);
		await rename(pending, this.path);
	}
}

/** `$XDG_CONFIG_HOME`, or `~/.config` when it is unset or empty. */
function configHome(): string {
	const configured = process.env.XDG_CONFIG_HOME;
	return configured !== undefined && configured !== '' ? configured : join(homedir(), '.config');
}

/** A Desktop Entry for `executable`; its Exec value is quoted, then string-escaped, as the specification requires. */
export function autostartEntry(executable: string): string {
	if (/[\n\r]/.test(executable)) {
		throw new Error('The executable path contains a line break');
	}
	const quoted = executable.replace(/[\\"`$]/g, character => `\\${character}`).replace(/%/g, '%%').replace(/\\/g, '\\\\');
	return `[Desktop Entry]\nType=Application\nName=${product.name}\nExec="${quoted}" ${HIDDEN_ARGUMENT}\nTerminal=false\nX-GNOME-Autostart-enabled=true\n`;
}

export function platformLoginItem(): LoginItem {
	return isLinux ? new AutostartLoginItem() : new ElectronLoginItem();
}

/** Whether this launch was started by the login item rather than by the user. */
export function launchedAtLogin(argv: readonly string[] = process.argv): boolean {
	return isMacintosh ? app.getLoginItemSettings().wasOpenedAtLogin : argv.includes(HIDDEN_ARGUMENT);
}

/**
 * Settings the operating system owns. Each value is read from the OS, so a
 * change made in the system's own settings is what the app shows.
 */
export class SystemSettings extends Disposable {
	private readonly changed = this._register(new Emitter<void>());
	public readonly onDidChange = this.changed.event;
	private openAtLogin = false;

	constructor(private readonly loginItem: LoginItem = platformLoginItem()) {
		super();
	}

	public get values(): Readonly<Record<string, unknown>> {
		return { 'application.openAtLogin': this.openAtLogin };
	}

	/** Settings this launch cannot change. `application.runInBackground` is stored in settings.json but listed here with the other application behavior. */
	public get restricted(): readonly string[] {
		return [...(canRunInBackground ? [] : ['application.runInBackground']), ...(this.loginItem.supported ? [] : ['application.openAtLogin'])];
	}

	public async refresh(): Promise<void> {
		const openAtLogin = this.loginItem.supported && await this.loginItem.read();
		if (openAtLogin !== this.openAtLogin && !this._store.isDisposed) {
			this.openAtLogin = openAtLogin;
			this.changed.fire();
		}
	}

	public async update(key: string, value: unknown): Promise<void> {
		if (key !== 'application.openAtLogin' || !this.loginItem.supported) {
			throw new Error(`${key} cannot be changed here`);
		}
		await this.loginItem.write(value === true);
		await this.refresh();
	}
}
