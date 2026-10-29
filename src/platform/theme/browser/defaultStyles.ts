import type { IButtonStyles } from '../../../base/browser/ui/button/button.js';
import type { ICodeInputStyles } from '../../../base/browser/ui/codeInput/codeInput.js';
import type { IDialogStyles } from '../../../base/browser/ui/dialog/dialog.js';
import type { IInputBoxStyles } from '../../../base/browser/ui/inputbox/inputBox.js';
import type { IListStyles } from '../../../base/browser/ui/list/listWidget.js';
import type { IMenuStyles } from '../../../base/browser/ui/menu/menu.js';
import type { ICheckboxStyles } from '../../../base/browser/ui/toggle/toggle.js';

/**
 * Styles for VS Code's widgets, in the role of VS Code's `defaultStyles.ts`.
 * VniLink's palette is the CSS custom properties in `colors.css`, so each
 * style names a variable from there instead of a color registry id.
 */

export const defaultButtonStyles: IButtonStyles = {
	buttonForeground: 'var(--on-accent)',
	buttonSeparator: 'var(--on-accent)',
	buttonBackground: 'var(--accent)',
	buttonHoverBackground: 'var(--accent)',
	buttonBorder: 'var(--accent)',
	buttonSecondaryForeground: 'var(--text)',
	buttonSecondaryBackground: 'transparent',
	buttonSecondaryHoverBackground: 'var(--surface-hover)',
	buttonSecondaryBorder: 'var(--line)',
};

export const defaultCodeInputStyles: ICodeInputStyles = {
	background: 'var(--canvas)',
	foreground: 'var(--text)',
	border: 'var(--line)',
	focusBorder: 'var(--accent)',
};

export const defaultInputBoxStyles: IInputBoxStyles = {
	inputBackground: 'var(--canvas)',
	inputForeground: 'var(--text)',
	inputBorder: 'var(--line)',
	inputValidationInfoBorder: undefined,
	inputValidationInfoBackground: undefined,
	inputValidationInfoForeground: undefined,
	inputValidationWarningBorder: undefined,
	inputValidationWarningBackground: undefined,
	inputValidationWarningForeground: undefined,
	inputValidationErrorBorder: undefined,
	inputValidationErrorBackground: undefined,
	inputValidationErrorForeground: undefined,
};

/** VS Code's dialog, in the colours of the surface it floats over. */
export const defaultDialogStyles: IDialogStyles = {
	dialogForeground: 'var(--text)',
	dialogBackground: 'var(--surface-raised)',
	dialogShadow: 'rgb(0 0 0 / 30%)',
	dialogBorder: 'var(--line)',
	errorIconForeground: 'var(--status-disconnected)',
	warningIconForeground: 'var(--status-pending)',
	infoIconForeground: 'var(--accent)',
	textLinkForeground: 'var(--accent)',
};

export const defaultCheckboxStyles: ICheckboxStyles = {
	checkboxBackground: 'var(--canvas)',
	checkboxBorder: 'var(--line)',
	checkboxForeground: 'var(--text)',
	checkboxDisabledBackground: undefined,
	checkboxDisabledForeground: undefined,
};

/**
 * Lists draw focus, selection and hover in their feature's stylesheet, so
 * the widget generates no rules of its own (and needs no inline styles).
 */
export const defaultListStyles: IListStyles = {
	listBackground: undefined,
	listFocusBackground: undefined,
	listFocusForeground: undefined,
	listActiveSelectionBackground: undefined,
	listActiveSelectionForeground: undefined,
	listActiveSelectionIconForeground: undefined,
	listFocusAndSelectionOutline: undefined,
	listFocusAndSelectionBackground: undefined,
	listFocusAndSelectionForeground: undefined,
	listInactiveSelectionBackground: undefined,
	listInactiveSelectionIconForeground: undefined,
	listInactiveSelectionForeground: undefined,
	listInactiveFocusForeground: undefined,
	listInactiveFocusBackground: undefined,
	listHoverBackground: undefined,
	listHoverForeground: undefined,
	listDropOverBackground: undefined,
	listDropBetweenBackground: undefined,
	listFocusOutline: undefined,
	listInactiveFocusOutline: undefined,
	listSelectionOutline: undefined,
	listHoverOutline: undefined,
	treeIndentGuidesStroke: undefined,
	treeInactiveIndentGuidesStroke: undefined,
	treeStickyScrollBackground: undefined,
	treeStickyScrollBorder: undefined,
	treeStickyScrollShadow: undefined,
	tableColumnsBorder: undefined,
	tableOddRowsBackgroundColor: undefined,
};

/**
 * Menus float over the window like the dialog; the item under the pointer or
 * keyboard is drawn in the accent, as the system's own menus are.
 */
export const defaultMenuStyles: IMenuStyles = {
	shadowColor: undefined,
	borderColor: 'var(--line)',
	foregroundColor: 'var(--text)',
	backgroundColor: 'var(--surface-raised)',
	selectionForegroundColor: 'var(--on-accent)',
	selectionBackgroundColor: 'var(--accent)',
	selectionBorderColor: undefined,
	separatorColor: 'var(--line)',
	scrollbarShadow: undefined,
	scrollbarSliderBackground: undefined,
	scrollbarSliderHoverBackground: undefined,
	scrollbarSliderActiveBackground: undefined,
};
