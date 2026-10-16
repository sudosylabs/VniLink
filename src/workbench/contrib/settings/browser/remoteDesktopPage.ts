import * as nls from '../../../../nls.js';
import { Disposable } from '../../../../base/common/lifecycle.js';
import { IInstantiationService } from '../../../../platform/instantiation/common/instantiation.js';
import type { TabAction } from '../../../services/tabs/common/tabService.js';
import { createPageHeading, type WorkbenchPage } from '../../../browser/parts/views/page.js';
import { DesktopDefaultsSection } from './desktopDefaultsSection.js';
import { DesktopCodecSection } from './desktopCodecSection.js';
import './media/remoteDesktop.css';

/** Global preferences; session controls belong to the remote desktop view. */
export class RemoteDesktopPage extends Disposable implements WorkbenchPage {
	public readonly element = document.createElement('main');
	public readonly actions: readonly TabAction[] = [];

	constructor(@IInstantiationService instantiation: IInstantiationService) {
		super();
		this.element.className = 'remote-desktop-page';
		this.element.dataset.settingsPage = 'desktop';
		this.element.append(createPageHeading(nls.localize('home.desktop', 'Remote Desktop'), nls.localize('desktop.description', 'Choose the picture quality and codec used for new remote desktop connections.')));
		const content = document.createElement('div');
		content.className = 'desktop-content';
		content.append(this._register(instantiation.createInstance(DesktopDefaultsSection)).element, this._register(instantiation.createInstance(DesktopCodecSection)).element);
		this.element.append(content);
	}

	public override dispose(): void {
		super.dispose();
		this.element.remove();
	}
}
