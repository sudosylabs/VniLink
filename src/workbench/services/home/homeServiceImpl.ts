// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import { Emitter } from '../../../base/common/event.js';
import { Disposable } from '../../../base/common/lifecycle.js';
import { InstantiationType, registerSingleton } from '../../../platform/instantiation/common/extensions.js';
import { ITabService } from '../tabs/tabService.js';
import { HOME_TAB_ID, IHomeService } from './homeService.js';

class HomeService extends Disposable implements IHomeService {
	declare public readonly _serviceBrand: undefined;
	private readonly requested = this._register(new Emitter<string>());
	public readonly onDidRequestPage = this.requested.event;
	private page: string | undefined;

	constructor(@ITabService private readonly tabService: ITabService) {
		super();
	}

	public get requestedPage(): string | undefined {
		return this.page;
	}

	public showPage(id: string): void {
		this.page = id;
		if (this.tabService.tabs.some(tab => tab.id === HOME_TAB_ID)) {
			this.tabService.select(HOME_TAB_ID);
		}
		this.requested.fire(id);
	}
}

registerSingleton(IHomeService, HomeService, InstantiationType.Delayed);
