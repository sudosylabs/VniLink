/**
 * Reads the window id from an IPC connection context. The Electron IPC server
 * assigns `window:<id>` from the sender, so the id names the calling window.
 */
export function windowIdFromContext(context: string): number {
	const match = /^window:([1-9][0-9]{0,8})$/.exec(context);
	if (!match) {
		throw new Error('IPC request did not come from an application window');
	}
	return Number(match[1]);
}
