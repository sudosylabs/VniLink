// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import type { IconId, IconVariant } from '../../../common/icons.js';
import { renderIcon } from '../icon/icon.js';

export interface ButtonOptions {
	icon?: IconId;
	variant?: IconVariant;
	onClick?: () => void;
}

export function button(label: string, className: string, options: ButtonOptions = {}): HTMLButtonElement {
	const element = document.createElement('button');
	element.type = 'button';
	element.className = className;
	element.textContent = label;
	if (options.icon) {
		element.prepend(renderIcon(options.icon, { variant: options.variant }));
	}
	if (options.onClick) {
		element.addEventListener('click', options.onClick);
	}
	return element;
}
