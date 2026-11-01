/*---------------------------------------------------------------------------------------------
 *  Copyright (c) Microsoft Corporation. All rights reserved.
 *  Licensed under the MIT License. See LICENSE.vscode.txt in the project root for license information.
 *--------------------------------------------------------------------------------------------*/

import type { IHoverDelegate } from './hoverDelegate.js';
import type { IMarkdownString } from '../../../common/htmlContent.js';
import type { IDisposable } from '../../../common/lifecycle.js';

/**
 * Enables the convenient display of rich markdown-based hovers in the workbench.
 */
export interface IHoverDelegate2 {
	/**
	 * Shows a hover after a delay, or immediately if the {@link groupId} matches the currently
	 * shown hover.
	 *
	 * Use this method when you want to:
	 *
	 * - Control showing the hover yourself.
	 * - Show the hover after the standard delay.
	 *
	 * @param options The options of the hover.
	 * @param groupId The group ID of the hover. If the group ID is the same as the currently shown
	 * hover, the hover will be shown immediately, skipping the delay.
	 */
	showDelayedHover(
		options: IHoverOptions,
		lifecycleOptions: Pick<IHoverLifecycleOptions, 'groupId'>,
	): IHoverWidget | undefined;

	/**
	 * A simple wrapper around showDelayedHover that includes listening to events on the
	 * {@link target} element that shows the hover.
	 *
	 * Use this method when you want to:
	 *
	 * - Let the hover service handle showing the hover.
	 * - Show the hover after the standard delay.
	 * - Want the hover positioned beside the {@link target} element.
	 *
	 * @param target The target element to listener for mouseover events on.
	 * @param hoverOptions The options of the hover.
	 * @param lifecycleOptions The options of the hover's lifecycle.
	 */
	setupDelayedHover(
		target: HTMLElement,
		hoverOptions: (() => IDelayedHoverOptions) | IDelayedHoverOptions,
		lifecycleOptions?: IHoverLifecycleOptions,
	): IDisposable;

	/**
	 * A simple wrapper around showDelayedHover that includes listening to events on the
	 * {@link target} element that shows the hover. This differs from {@link setupDelayedHover} in
	 * that the hover will be shown at the mouse position instead of the
	 * {@link target target} element's position, ignoring any
	 * {@link IHoverOptions.position position options} that are passed in.
	 *
	 * Use this method when you want to:
	 *
	 * - Let the hover service handle showing the hover.
	 * - Show the hover after the standard delay.
	 * - Want the hover positioned beside the mouse.
	 *
	 * @param target The target element to listener for mouseover events on.
	 * @param hoverOptions The options of the hover.
	 * @param lifecycleOptions The options of the hover's lifecycle.
	 */
	setupDelayedHoverAtMouse(
		target: HTMLElement,
		hoverOptions: (() => IDelayedHoverAtMouseOptions) | IDelayedHoverAtMouseOptions,
		lifecycleOptions?: IHoverLifecycleOptions,
	): IDisposable;

	/**
	 * Shows a hover immediately, provided a hover with the same {@link options} object is not
	 * already visible.
	 *
	 * Use this method when you want to:
	 *
	 * - Control showing the hover yourself.
	 * - Show the hover immediately.
	 *
	 * @param options A set of options defining the characteristics of the hover.
	 * @param focus Whether to focus the hover (useful for keyboard accessibility).
	 *
	 * @example A simple usage with a single element target.
	 *
	 * ```typescript
	 * showInstantHover({
	 *   text: new MarkdownString('Hello world'),
	 *   target: someElement
	 * });
	 * ```
	 */
	showInstantHover(
		options: IHoverOptions,
		focus?: boolean
	): IHoverWidget | undefined;

	/**
	 * Hides the hover if it was visible. This call will be ignored if the hover is currently
	 * "locked" via the alt/option key unless `force` is set.
	 */
	hideHover(force?: boolean): void;

	/**
	 * This should only be used until we have the ability to show multiple context views
	 * simultaneously. #188822
	 */
	showAndFocusLastHover(): void;

	/**
	 * Sets up a managed hover for the given element. A managed hover will set up listeners for
	 * mouse events, show the hover after a delay and provide hooks to easily update the content.
	 *
	 * This should be used over {@link showInstantHover} when fine-grained control is not needed. The
	 * managed hover also does not scale well, consider using {@link showInstantHover} when showing hovers
	 * for many elements.
	 *
	 * @param hoverDelegate The hover delegate containing hooks and configuration for the hover.
	 * @param targetElement The target element to show the hover for.
	 * @param content The content of the hover or a factory that creates it at the time it's shown.
	 * @param options Additional options for the managed hover.
	 *
	 * @deprecated Use {@link setupDelayedHover} or {@link setupDelayedHoverAtMouse} instead where
	 * possible.
	 */
	setupManagedHover(hoverDelegate: IHoverDelegate, targetElement: HTMLElement, content: IManagedHoverContentOrFactory, options?: IManagedHoverOptions): IManagedHover;

	/**
	 * Shows the hover for the given element if one has been setup.
	 *
	 * @param targetElement The target element of the hover, as set up in {@link setupManagedHover}.
	 *
	 * @deprecated Use {@link setupDelayedHover} or {@link setupDelayedHoverAtMouse} instead where
	 * possible.
	 */
	showManagedHover(targetElement: HTMLElement): void;
}

export interface IHoverWidget extends IDisposable {
}

export const enum HoverStyle {
	/**
	 * The hover is anchored below the element with a pointer above it pointing at the target.
	 */
	Pointer = 1,
	/**
	 * The hover is anchored to the bottom right of the cursor's location.
	 */
	Mouse = 2,
}

export interface IHoverOptions {
	/**
	 * The content to display in the primary section of the hover. The type of text determines the
	 * default `hideOnHover` behavior.
	 */
	content: IMarkdownString | string | HTMLElement;

	/**
	 * The target for the hover. This determines the position of the hover and it will only be
	 * hidden when the mouse leaves both the hover and the target. A HTMLElement can be used for
	 * simple cases and a IHoverTarget for more complex cases where multiple elements and/or a
	 * dispose method is required.
	 */
	target: IHoverTarget | HTMLElement;

	/**
	 * A set of actions for the hover's "status bar".
	 */
	actions?: IHoverAction[];

	/**
	 * An optional array of classes to add to the hover element.
	 */
	additionalClasses?: string[];

	/**
	 * An optional link handler for markdown links, if this is not provided the IOpenerService will
	 * be used to open the links using its default options.
	 */
	linkHandler?(url: string): void;

	/**
	 * Whether to trap focus in the following ways:
	 * - When the hover closes, focus goes to the element that had focus before the hover opened
	 * - If there are elements in the hover to focus, focus stays inside of the hover when tabbing
	 * Note that this is overridden to true when in screen reader optimized mode.
	 */
	trapFocus?: boolean;

	/**
	 * The style of the hover, this sets default values of {@link position} and {@link appearance}:
	 */
	style?: HoverStyle;

	/**
	 * Options that defines where the hover is positioned.
	 */
	position?: IHoverPositionOptions;

	/**
	 * Options that define how the hover looks.
	 */
	appearance?: IHoverAppearanceOptions;

	/**
	 * An optional callback that is called when the hover is shown. This is called
	 * later for delayed hovers.
	 */
	onDidShow?(): void;
}

// `target` is ignored for delayed hover methods as it's included in the method and added
// automatically when the hover options get resolved.
export type IDelayedHoverOptions = Omit<IHoverOptions, 'target'>;

// `position` is ignored for delayed at mouse hover methods as it's overwritten by the mouse event.
// `showPointer` is always false when using mouse positioning
export type IDelayedHoverAtMouseOptions = Omit<IDelayedHoverOptions, 'position' | 'appearance'> & { appearance?: Omit<IHoverAppearanceOptions, 'showPointer'> };

export interface IHoverLifecycleOptions {
	/**
	 * The group ID of the hover. If the group ID is the same as the currently shown hover, the
	 * hover will be shown immediately, skipping the delay.
	 *
	 * @example Use a UUID to set a unique `groupId` for related hovers
	 *
	 * ```typescript
	 * const groupId = generateUuid();
	 * showDelayedHover({ content: 'Button 1', target: someElement1 }, { groupId });
	 * showDelayedHover({ content: 'Button 2', target: someElement2 }, { groupId });
	 * ```
	 *
	 * @example Use a feature-specific string to set a unqiue `groupId` for related hovers
	 *
	 * ```typescript
	 * showDelayedHover({ content: 'Button 1', target: someElement1 }, { groupId: 'my-feature-items' });
	 * showDelayedHover({ content: 'Button 2', target: someElement2 }, { groupId: 'my-feature-items' });
	 * ```
	 */
	groupId?: string;
}

export interface IHoverPositionOptions {
}

export interface IHoverAppearanceOptions {
	/**
	 * Whether to show the hover pointer, a little arrow that connects the target and the hover.
	 */
	showPointer?: boolean;

	/**
	 * Whether to show a compact hover, reducing the font size and padding of the hover.
	 */
	compact?: boolean;
}

export interface IHoverAction {
}

/**
 * A target for a hover.
 */
export interface IHoverTarget extends Partial<IDisposable> {
}

// #region Managed hover

export interface IManagedHoverTooltipMarkdownString {
}

export interface IManagedHoverTooltipHTMLElement {
}

export type IManagedHoverContent = string | IManagedHoverTooltipMarkdownString | IManagedHoverTooltipHTMLElement | HTMLElement | undefined;
export type IManagedHoverContentOrFactory = IManagedHoverContent | (() => IManagedHoverContent);

export interface IManagedHoverOptions extends Pick<IHoverOptions, 'actions' | 'additionalClasses' | 'linkHandler' | 'trapFocus' | 'onDidShow'> {
}

export interface IManagedHover extends IDisposable {
	/**
	 * Allows to programmatically open the hover.
	 */
	show(focus?: boolean): void;

	/**
	 * Allows to programmatically hide the hover.
	 */
	hide(): void;

	/**
	 * Updates the contents of the hover.
	 */
	update(tooltip: IManagedHoverContent, options?: IManagedHoverOptions): void;
}

// #endregion Managed hover
