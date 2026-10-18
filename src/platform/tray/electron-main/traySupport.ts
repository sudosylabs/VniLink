import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { isLinux } from '../../../base/common/platform.js';

type Run = (command: string, args: readonly string[]) => Promise<{ stdout: string }>;

const execute = promisify(execFile);

const run: Run = (command, args) => execute(command, args, { timeout: 2000 });

/**
 * Whether this desktop shows tray icons. macOS and Windows always do. Linux
 * panels show Electron's StatusNotifierItem only while a watcher owns its bus
 * name; GNOME has none without the AppIndicator extension, and an icon nobody
 * shows would leave a windowless VniLink with no way to quit.
 */
export async function traySupported(runCommand: Run = run, linux = isLinux): Promise<boolean> {
	if (!linux) {
		return true;
	}
	try {
		const { stdout } = await runCommand('gdbus', ['call', '--session', '--dest', 'org.freedesktop.DBus', '--object-path', '/org/freedesktop/DBus', '--method', 'org.freedesktop.DBus.NameHasOwner', 'org.kde.StatusNotifierWatcher']);
		return stdout.trim().startsWith('(true');
	} catch {
		return false;
	}
}
