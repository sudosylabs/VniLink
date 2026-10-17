import { Disposable } from '../base/common/lifecycle.js';
import type { IThemeService } from '../platform/theme/common/themeService.js';

/**
 * A region of the workbench, in the shape of VS Code's `Part`: the workbench
 * creates it into a container, it builds its content there, and it restyles
 * itself whenever the theme changes. Parts own their DOM and listeners.
 */
export abstract class Part extends Disposable {
	private parent: HTMLElement | undefined;

	constructor(
		public readonly id: string,
		protected readonly themeService: IThemeService,
	) {
		super();
		this._register(themeService.onDidColorThemeChange(() => {
			if (this.parent) {
				this.updateStyles();
			}
		}));
	}

	/** Called once by the workbench to build the part into `parent`. */
	public create(parent: HTMLElement): void {
		this.parent = parent;
		this.createContentArea(parent);
		this.updateStyles();
	}

	/** The overall part container, once created. */
	public getContainer(): HTMLElement | undefined {
		return this.parent;
	}

	protected abstract createContentArea(parent: HTMLElement): HTMLElement;

	/** Applies theme-dependent styles; parts override this rather than listening themselves. */
	protected updateStyles(): void {
		// Parts without theme-dependent styles have nothing to update.
	}
}
