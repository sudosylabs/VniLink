import * as nls from '../../../../nls.js';
import { Disposable } from '../../../../base/common/lifecycle.js';
import { IInstantiationService } from '../../../../platform/instantiation/common/instantiation.js';
import type { TabAction } from '../../../services/tabs/common/tabService.js';
import { createPageHeading, type WorkbenchPage } from '../../../browser/parts/views/page.js';
import { SharingSection } from './sharingSection.js';
import { LocalSecuritySection } from './localSecuritySection.js';
import { MachinePermissionsSection } from './machinePermissionsSection.js';
import './media/access.css';

export class AccessPage extends Disposable implements WorkbenchPage {
	public readonly element = document.createElement('main');
	public readonly actions: readonly TabAction[] = [];

	constructor(@IInstantiationService instantiation: IInstantiationService) {
		super();
		this.element.className = 'access-page';
		this.element.append(createPageHeading(nls.localize('home.access', 'Access & Security'), nls.localize('access.description', 'Manage the app’s system permissions and the actions remote devices may request.')));
		const content = document.createElement('div');
		content.className = 'access-content';
		content.append(this._register(instantiation.createInstance(SharingSection)).element, this._register(instantiation.createInstance(MachinePermissionsSection)).element, this._register(instantiation.createInstance(LocalSecuritySection)).element);
		this.element.append(content);
	}

	public override dispose(): void {
		super.dispose();
		this.element.remove();
	}
}
