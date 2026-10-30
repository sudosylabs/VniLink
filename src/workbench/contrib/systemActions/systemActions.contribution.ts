// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import { registerAction2 } from '../../../platform/actions/common/actions.js';
import { CancelSystemActionAction, LockComputerAction, RestartOrShutDownAction } from './systemActionsActions.js';

registerAction2(LockComputerAction);

registerAction2(RestartOrShutDownAction);

registerAction2(CancelSystemActionAction);
