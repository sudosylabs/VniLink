// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

/**
 * The operating system the build runs on, for build code, which runs before
 * or without the application's modules: `src/base/common/platform.ts`
 * holds the application's own helpers, which Node cannot load directly.
 */

export const isWindows = process.platform === 'win32';

export const isMacintosh = process.platform === 'darwin';

export const isLinux = process.platform === 'linux';

/** This OS by the name the test kit reports another computer's (`std::env::consts::OS`). */
export const osName: string = isMacintosh ? 'macos' : isWindows ? 'windows' : process.platform;
