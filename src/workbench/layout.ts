import * as nls from '../nls.js';
import { $, append } from '../base/browser/dom.js';
import { setARIAContainer } from '../base/browser/ui/aria/aria.js';
import { Disposable } from '../base/common/lifecycle.js';
import type { Part } from './part.js';

/**
 * Arranges the workbench's parts in the window, in the role of VS Code's
 * `Layout`: the titlebar above a workspace that holds the notice line and the
 * views part. The workbench decides what the parts are.
 */
export abstract class Layout extends Disposable {
	private readonly titlebarContainer: HTMLElement;
	private readonly workspace: HTMLElement;
	private readonly notice: HTMLElement;
	private readonly viewsContainer: HTMLElement;

	constructor(protected readonly parent: HTMLElement) {
		super();
		this.titlebarContainer = $('header');
		this.workspace = $('.workspace');
		this.notice = append(this.workspace, $('.workbench-notice', { role: 'alert' }));
		this.notice.hidden = true;
		this.viewsContainer = append(this.workspace, $('div'));
		this._register({
			dispose: () => {
				this.titlebarContainer.remove();
				this.workspace.remove();
			},
		});
	}

	protected createWorkbenchLayout(titlebar: Part, views: Part): void {
		titlebar.create(this.titlebarContainer);
		views.create(this.viewsContainer);
		setARIAContainer(this.workspace);
		this.parent.replaceChildren(this.titlebarContainer, this.workspace);
	}

	/** Shows a message above the views; `undefined` hides it. */
	protected setNotice(message: string | undefined): void {
		this.notice.hidden = message === undefined;
		this.notice.textContent = message ?? '';
	}

	protected static readonly engineUnavailableNotice = nls.localize('workbench.engineUnavailable', 'VniLink could not start its connection service. Restart the app to try again.');
	protected static readonly unexpectedErrorNotice = nls.localize('workbench.actionFailed', 'The action could not be completed. Try again.');
}
