// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import { registerMainProcessRemoteService } from '../../ipc/electron-browser/services.js';
import { IRecordingService, RECORDING_CHANNEL_NAME } from '../common/recording.js';

registerMainProcessRemoteService(IRecordingService, RECORDING_CHANNEL_NAME, { proxyOptions: { disableMarshalling: true } });
