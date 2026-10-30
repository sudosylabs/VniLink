// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import { upstreamIgnores } from './build/sourceScope.ts';

export default {
	ignoreFiles: upstreamIgnores,
	extends: ['stylelint-config-standard'],
	rules: {
		'property-no-unknown': [true, { ignoreProperties: ['app-region', '-webkit-app-region'] }],
		'custom-property-pattern': '^[a-z][a-z0-9]*(-[a-z0-9]+)*$',
		'selector-class-pattern': '^[a-z][a-z0-9]*(-[a-z0-9]+)*$',
	},
};
