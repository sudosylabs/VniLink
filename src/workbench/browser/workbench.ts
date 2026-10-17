/** Owns workbench parts, service lifetime and startup; native composition is supplied by DesktopMain. */
import { setARIAContainer } from '../../base/browser/ui/aria/aria.js';
import { addDisposableListener } from '../../base/browser/domEvent.js';
import { ensureCodeWindow } from '../../base/browser/window.js';
import { Disposable, DisposableStore, type IDisposable, toDisposable } from '../../base/common/lifecycle.js';
import { IConfigurationService } from '../../platform/configuration/common/configuration.js';
import { getSingletonServiceDescriptors } from '../../platform/instantiation/common/extensions.js';
import { InstantiationService } from '../../platform/instantiation/common/instantiationService.js';
import type { ServiceCollection } from '../../platform/instantiation/common/serviceCollection.js';
import type { INativeWindowConfiguration } from '../../platform/window/common/windowConfiguration.js';
import * as nls from '../../nls.js';
import { HomeView } from '../contrib/home/browser/home.js';
import { registerWindowDriver } from '../services/driver/browser/driver.js';
import { ITabService } from '../services/tabs/common/tabService.js';
import { ViewHost } from './parts/views/viewHost.js';
import './styles/workbench.css';

export interface IWorkbenchTitlebar extends IDisposable {
	create(parent: HTMLElement): void;
	initialize(): Promise<void>;
}

export class Workbench extends Disposable {
	private readonly parts = this._register(new DisposableStore());
	private readonly notice = document.createElement('div');
	private readonly header = document.createElement('header');
	private readonly workspace = document.createElement('div');
	private services: InstantiationService | undefined;
	private restored = false;
	private startupPromise: Promise<void> | undefined;

	constructor(
		private readonly parent: HTMLElement,
		private readonly info: INativeWindowConfiguration,
		private readonly serviceCollection: ServiceCollection,
		private readonly createTitlebar: (services: InstantiationService, onError: (error: unknown) => void) => IWorkbenchTitlebar,
	) {
		super();
		this._register(addDisposableListener(window, 'pagehide', () => {
			this.dispose();
		}));
		this._register(toDisposable(() => {
			if (this.restored) {
				delete document.documentElement.dataset.workbenchReady;
			}
			this.header.remove();
			this.workspace.remove();
		}));
	}

	public startup(): Promise<void> {
		this.startupPromise ??= this.start().catch((error: unknown) => {
			this.dispose();
			throw error;
		});
		return this.startupPromise;
	}

	private async start(): Promise<void> {
		this.checkAlive();
		ensureCodeWindow(window, this.info.windowId);
		const services = this.initServices();
		await services.invokeFunction(accessor => accessor.get(IConfigurationService)).initialize();
		this.checkAlive();
		const titlebar = this.parts.add(this.createTitlebar(services, error => {
			this.showError(error);
		}));
		this.renderWorkbench(services, titlebar);
		await titlebar.initialize();
		this.checkAlive();
		if (this.info.enableSmokeTestDriver) {
			registerWindowDriver(services);
		}
		this.restored = true;
		document.documentElement.dataset.workbenchReady = 'true';
	}

	private initServices(): InstantiationService {
		for (const [id, descriptor] of getSingletonServiceDescriptors()) {
			this.serviceCollection.set(id, descriptor);
		}
		this.services = this._register(new InstantiationService(this.serviceCollection, true));
		return this.services;
	}

	private renderWorkbench(services: InstantiationService, titlebar: IWorkbenchTitlebar): void {
		document.documentElement.dataset.platform = this.info.platform;
		this.notice.className = 'workbench-notice';
		this.notice.setAttribute('role', 'alert');
		this.notice.hidden = this.info.engineAvailable;
		this.notice.textContent = nls.localize('workbench.engineUnavailable', 'VniLink could not start its connection service. Restart the app to try again.');
		this.workspace.className = 'workspace';
		const viewsContainer = document.createElement('div');
		const viewHost = this.parts.add(services.createInstance(ViewHost, viewsContainer));
		const content = document.createElement('div');
		content.className = 'view-stack';
		content.append(viewsContainer);
		this.workspace.append(this.notice, content);
		setARIAContainer(this.workspace);
		titlebar.create(this.header);
		this.parent.replaceChildren(this.header, this.workspace);
		viewHost.open(services.createInstance(HomeView));
		if (this.info.engineAvailable) {
			const tabs = services.invokeFunction(accessor => accessor.get(ITabService));
			void tabs.executeAction('refresh').catch((error: unknown) => {
				if (!this._store.isDisposed) {
					this.showError(error);
				}
			});
		}
	}

	private checkAlive(): void {
		if (this._store.isDisposed) {
			throw new Error('Workbench disposed during startup');
		}
	}

	private showError(error: unknown): void {
		console.error(error);
		this.notice.hidden = false;
		this.notice.textContent = nls.localize('workbench.actionFailed', 'The action could not be completed. Try again.');
	}
}
