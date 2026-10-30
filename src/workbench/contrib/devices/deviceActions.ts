// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import { localize2 } from '../../../nls.js';
import { Icons } from '../../../base/common/icons.js';
import { ThemeIcon } from '../../../base/common/themables.js';
import { Action2 } from '../../../platform/actions/common/actions.js';
import type { ServicesAccessor } from '../../../platform/instantiation/common/instantiation.js';
import { EngineAvailableContext } from '../../contextkeys.js';
import { isViewActionContext } from '../../parts/titlebar/viewActionsToolbar.js';
import { IDevicePairingPopover, type PairingPopoverKind } from './devicePairingPopover.js';

abstract class PairingAction extends Action2 {
	protected abstract readonly kind: PairingPopoverKind;

	public override run(accessor: ServicesAccessor, context?: unknown): void {
		// Run from the Devices page, the popover hangs from the button; nothing else has an anchor.
		if (isViewActionContext(context)) {
			accessor.get(IDevicePairingPopover).toggle(context.anchor, this.kind);
		}
	}
}

/** Opens a .vnl invitation another computer shared. */
export class ImportInvitationAction extends PairingAction {
	public static readonly ID = 'vnilink.devices.importInvitation';
	protected readonly kind = 'import';

	constructor() {
		super({
			id: ImportInvitationAction.ID,
			title: localize2('devices.importInvitation', 'Open an invitation'),
			icon: ThemeIcon.fromId(Icons.importInvitation),
			precondition: EngineAvailableContext,
		});
	}
}

/** Creates an invitation for this computer to pair with another. */
export class CreateInvitationAction extends PairingAction {
	public static readonly ID = 'vnilink.devices.createInvitation';
	protected readonly kind = 'create';

	constructor() {
		super({
			id: CreateInvitationAction.ID,
			title: localize2('devices.createInvitation', 'Invite another computer'),
			icon: ThemeIcon.fromId(Icons.createInvitation),
			precondition: EngineAvailableContext,
		});
	}
}
