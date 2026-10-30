// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import metadata from '../../../../product.json' with { type: 'json' };

/** Runtime identity is derived from the same metadata as packaged application resources. */
export const product = Object.freeze({
	name: metadata.nameLong,
	appId: metadata.appId,
});
