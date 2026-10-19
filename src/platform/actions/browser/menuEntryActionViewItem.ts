import { ModifierKeyEmitter } from '../../../base/browser/dom.js';
import { type IAction, Separator } from '../../../base/common/actions.js';
import { isLinux, isWindows } from '../../../base/common/platform.js';
import { MenuItemAction, type SubmenuItemAction } from '../common/actions.js';

/**
 * Menu entry helpers, in the role of VS Code's `menuEntryActionViewItem.ts`.
 * VniLink renders its toolbar items itself (`viewActionsToolbar.ts`), so only
 * the flattening that context menus use is provided here.
 */

/**
 * Flattens a menu's groups into one list for a context menu, with a separator
 * between groups. While Alt (or Shift on Windows and Linux) is held, an item
 * with an alternative command shows that command instead, as in VS Code.
 */
export function getFlatContextMenuActions(groups: readonly [string, readonly (MenuItemAction | SubmenuItemAction)[]][]): IAction[] {
	const keyStatus = ModifierKeyEmitter.getInstance().keyStatus;
	const useAlternativeActions = keyStatus.altKey || ((isWindows || isLinux) && keyStatus.shiftKey);
	const target: IAction[] = [];
	for (const [, actions] of groups) {
		if (target.length > 0) {
			target.push(new Separator());
		}
		for (const action of actions) {
			target.push(useAlternativeActions && action instanceof MenuItemAction && action.alt ? action.alt : action);
		}
	}
	return target;
}
