/**
 * Whether this process runs an installed build rather than the source
 * checkout, as VS Code's `isBuilt`. A checkout is launched as
 * `electron <path>`, which Electron reports as `process.defaultApp`. Unlike
 * `app.isPackaged`, this does not depend on the executable's name, which the
 * development Electron (`build/lib/electron.ts`) shares with the product.
 */
export function isBuilt(): boolean {
	return !process.defaultApp;
}
