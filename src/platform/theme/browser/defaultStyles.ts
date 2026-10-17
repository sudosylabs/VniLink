import type { IButtonStyles } from '../../../base/browser/ui/button/button.js';
import type { IInputBoxStyles } from '../../../base/browser/ui/inputbox/inputBox.js';
import type { IListStyles } from '../../../base/browser/ui/list/listWidget.js';

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
