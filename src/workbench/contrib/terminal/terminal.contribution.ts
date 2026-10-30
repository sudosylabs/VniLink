// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import '@xterm/xterm/css/xterm.css';
import './media/terminal.css';
import { registerAction2 } from '../../../platform/actions/common/actions.js';
import { OpenTerminalAction } from './terminalActions.js';

registerAction2(OpenTerminalAction);
