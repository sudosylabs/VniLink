import * as nls from '../../../../nls.js';
import { Disposable } from '../../../../base/common/lifecycle.js';
import type { TabAction } from '../../../services/tabs/common/tabService.js';
import type { WorkbenchView } from '../../parts/views/viewHost.js';
import type { DesktopApi } from '../../../../platform/ipc/common/desktopApi.js';
import { createSidebar } from '../../parts/sidebar/sidebar.js';
import { Icons } from '../../../../base/common/icons.js';
import { renderIcon } from '../../../../base/browser/ui/icon/icon.js';

export function createComputersPage(): HTMLElement {
	const page = document.createElement('main');
	// This static markup contains no engine/device data. Future labels use textContent.
	page.innerHTML = `<header class="page-heading"><h1>${nls.localize('computers.heading', 'Your computers')}</h1><p>${nls.localize('computers.subheading', 'A place for the computers you connect to.')}</p></header>
    <section class="empty-state" aria-label="${nls.localize('computers.sectionLabel', 'Computers')}">
      <h2>${nls.localize('computers.emptyTitle', 'No computers yet')}</h2><p>${nls.localize('computers.emptyDescription', 'Paired computers will appear here.')}</p>
    </section>`;
	const icon = renderIcon(Icons.computers);
	icon.classList.add('computer');
	page.querySelector('.empty-state')?.prepend(icon);
	return page;
}

export class ComputersView extends Disposable implements WorkbenchView {
	public readonly id = 'computers';
	public readonly label = nls.localize('computers.viewLabel', 'Computers');
	public readonly icon = Icons.computers;
	public readonly element = document.createElement('div');
	public readonly actions: readonly TabAction[];

	constructor(
		private readonly api: DesktopApi,
		connected: boolean,
	) {
		super();
		this.element.className = 'computers-view';
		this.element.append(
			createSidebar(() => {
				this.element.querySelector('h1')?.scrollIntoView({ block: 'nearest' });
			}),
			createComputersPage(),
		);
		this.actions = [
			{
				id: 'refresh',
				label: nls.localize('computers.refreshLabel', 'Refresh computers'),
				icon: Icons.refresh,
				enabled: connected,
				run: async (signal) => {
					await this.refresh(signal);
				},
			},
		];
	}

	private async refresh(signal: AbortSignal): Promise<void> {
		const reply = await this.api.engineCommand({ type: 'snapshot' });
		if (signal.aborted || this._store.isDisposed) {
			return;
		}
		if (reply.type !== 'snapshot') {
			throw new Error('Expected engine snapshot');
		}
		const state = this.element.querySelector('.empty-state');
		if (!state) {
			return;
		}
		const fresh = createComputersPage().querySelector('.empty-state');
		if (!reply.snapshot.devices.length && fresh) {
			state.replaceChildren(...Array.from(fresh.childNodes));
			return;
		}
		state.replaceChildren();
		for (const device of reply.snapshot.devices) {
			const label = document.createElement('p');
			label.textContent = device.pet_name ?? device.hostname ?? device.endpoint_id;
			state.append(label);
		}
	}

	public override dispose(): void {
		this.element.remove();
		super.dispose();
	}
}
