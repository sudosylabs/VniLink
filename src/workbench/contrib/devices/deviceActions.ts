import { localize2 } from '../../../nls.js';
import { Icons } from '../../../base/common/icons.js';
import { ThemeIcon } from '../../../base/common/themables.js';
import { Action2 } from '../../../platform/actions/common/actions.js';
import { ContextKeyExpr } from '../../../platform/contextkey/common/contextkey.js';
import type { ServicesAccessor } from '../../../platform/instantiation/common/instantiation.js';
import { ActiveTabContext, EngineAvailableContext, HomePageContext } from '../../contextkeys.js';
import { isViewActionContext, ViewActionsMenu } from '../../parts/titlebar/viewActionsToolbar.js';
import { HomeView } from '../home/homeView.js';
import { IDevicePairingPopover, type PairingPopoverKind } from './devicePairingPopover.js';

/** Both pairing actions show on the Devices page only. */
const onDevicesPage = ContextKeyExpr.and(ActiveTabContext.isEqualTo(HomeView.ID), HomePageContext.isEqualTo('devices'));

abstract class PairingAction extends Action2 {
	protected abstract readonly kind: PairingPopoverKind;

	public override run(accessor: ServicesAccessor, context?: unknown): void {
		// Run from the titlebar, the popover hangs from the button; nothing else has an anchor.
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
			menu: { id: ViewActionsMenu, group: 'navigation', order: 1, when: onDevicesPage },
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
			menu: { id: ViewActionsMenu, group: 'navigation', order: 2, when: onDevicesPage },
		});
	}
}
