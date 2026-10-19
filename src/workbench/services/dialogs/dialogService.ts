import { createDecorator } from '../../../platform/instantiation/common/instantiation.js';

export const IDialogService = createDecorator<IDialogService>('dialogService');

/** A question with a few answers, in the role of VS Code's `IDialogService`. */
export interface IDialogOptions {
	readonly type: 'warning' | 'info' | 'error';
	readonly message: string;
	readonly detail?: string;
	/** The answers, in order; `cancelId` names the one that dismisses. */
	readonly buttons: readonly string[];
	readonly cancelId: number;
}

/**
 * Asks the person at this computer. macOS shows its native box, which
 * follows the system's appearance; elsewhere VniLink draws VS Code's dialog
 * in the app's theme, since a native Windows box is always light.
 */
export interface IDialogService {
	readonly _serviceBrand: undefined;
	/** Resolves with the index of the chosen button. */
	show(options: IDialogOptions): Promise<number>;
}
