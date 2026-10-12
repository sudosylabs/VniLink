import { TabService } from '../../src/workbench/services/tabs/common/tabServiceImpl.js';
import type { TabAction } from '../../src/workbench/services/tabs/common/tabService.js';
import { TitlebarTabsControl } from '../../src/workbench/browser/parts/titlebar/titlebarTabsControl.js';
import { ViewHost, type WorkbenchView } from '../../src/workbench/browser/parts/views/viewHost.js';
import { Emitter } from '../../src/base/common/event.js';
import { Icons } from '../../src/base/common/icons.js';
function assert(condition: boolean, message: string): void {
	if (!condition) {
		throw new Error(message);
	}
}
export async function verifyTabs(): Promise<boolean> {
	const service = new TabService();
	const fixture = document.createElement('div');
	fixture.className = 'part titlebar';
	fixture.style.cssText = 'position:fixed;left:0;top:80px;width:440px;height:35px;z-index:99';
	const root = document.createElement('div');
	root.className = 'titlebar-container';
	root.style.padding = '0';
	fixture.append(root);
	document.body.append(fixture);
	const content = document.createElement('div');
	fixture.append(content);
	const errors: unknown[] = [];
	const tabs = new TitlebarTabsControl(
		root,
		(error) => {
			errors.push(error);
		},
		service,
	);
	const host = new ViewHost(content, service);
	let disposed = 0;
	let finish: (() => void) | undefined;
	let signal: AbortSignal | undefined;
	const ran: string[] = [];
	const changed = new Emitter<void>();
	function view(id: string): WorkbenchView {
		const element = document.createElement('main');
		element.textContent = id;
		return {
			id,
			label: `View ${id}`,
			element,
			actions: [
				{
					id: 'action',
					label: `Action ${id}`,
					icon: Icons.refresh,
					run: () => {
						ran.push(id);
					},
				},
			],
			dispose: () => {
				disposed++;
				element.remove();
			},
		};
	}
	try {
		const first = view('first');
		const second = view('second');
		host.open(first);
		host.open(second);
		assert(first.element.hidden === true && second.element.hidden === false, 'Inactive view is not hidden');
		service.select(first.id);
		assert(
			first.element.hidden === false && second.element.hidden === true,
			'View state was not preserved on switching',
		);
		assert(disposed === 0, 'Switching disposed a view');
		const fill = root.querySelector<HTMLElement>('[aria-selected="true"] .tab-fill');
		assert(fill !== null && getComputedStyle(fill).borderTopLeftRadius === '6px', 'Modern tab fill is not rounded');
		assert(fill !== null && getComputedStyle(fill).borderBottomWidth === '0px', 'Connected active tab has a bottom divider');
		assert(fill !== null && getComputedStyle(fill).backgroundColor === getComputedStyle(document.body).backgroundColor || fill !== null && getComputedStyle(fill).backgroundColor === getComputedStyle(document.documentElement).backgroundColor, 'Active tab does not share the content surface');
		const action = root.querySelector<HTMLButtonElement>('.view-action');
		assert(action?.getAttribute('aria-label') === 'Action first', 'Toolbar does not follow active tab');
		action?.click();
		await Promise.resolve();
		await Promise.resolve();
		assert(ran.join() === 'first', 'Action ran against the wrong view');
		let busyEnabled = true;
		const busy: TabAction = {
			get enabled() {
				return busyEnabled;
			},
			id: 'busy',
			label: 'Busy',
			run: async (input) => {
				signal = input;
				await new Promise<void>((resolve) => {
					finish = resolve;
				});
				ran.push('busy');
			},
		};
		const dynamic: WorkbenchView = { ...view('dynamic'), actions: [busy], onDidChangeActions: changed.event };
		host.open(dynamic);
		busyEnabled = false;
		changed.fire();
		assert(
			root.querySelector<HTMLButtonElement>('.view-action')?.disabled === true,
			'Dynamic disable not reflected',
		);
		busyEnabled = true;
		changed.fire();
		assert(
			root.querySelector<HTMLButtonElement>('.view-action')?.disabled === false,
			'Dynamic enable not reflected',
		);
		const running = service.executeAction('busy');
		assert(
			root.querySelector<HTMLButtonElement>('.view-action')?.disabled === true,
			'Running action is not disabled',
		);
		service.select(second.id);
		assert(
			root.querySelector<HTMLButtonElement>('.view-action')?.getAttribute('aria-label') === 'Action second',
			'Toolbar retained previous actions',
		);
		await service.close(dynamic.id);
		assert(signal?.aborted === true, 'Closing did not cancel its running action');
		finish?.();
		await running;
		assert(service.activeTab === second, 'Old action changed active tab');
		const disabled = {
			id: 'disabled',
			label: 'Disabled',
			enabled: false,
			run: () => {
				throw new Error('Disabled action ran');
			},
		};
		const guarded = { ...view('guarded'), actions: [disabled], canClose: () => false };
		host.open(guarded);
		await service.executeAction('disabled');
		assert(!(await service.close(guarded.id)), 'Close veto ignored');
		for (let index = 0; index < 12; index++) {
			host.open(view(String(index)));
		}
		const scrollable = root.querySelector<HTMLElement>('.tabs-scrollable');
		const toolbar = root.querySelector<HTMLElement>('.view-actions');
		assert(
			scrollable !== null && toolbar !== null && scrollable.scrollWidth > scrollable.clientWidth,
			'Tabs do not overflow horizontally',
		);
		if (scrollable && toolbar) {
			assert(
				scrollable.getBoundingClientRect().right <= toolbar.getBoundingClientRect().left + 1,
				'Tabs overlap the contextual actions',
			);
			assert(scrollable.scrollLeft > 0, 'Active tab not revealed');
			scrollable.scrollLeft = 0;
			changed.fire();
			assert(scrollable.scrollLeft === 0, 'Action update moved the manually scrolled tab strip');
		}
		service.select('0');
		root.querySelector<HTMLElement>('[aria-selected="true"]')?.dispatchEvent(
			new KeyboardEvent('keydown', { key: 'ArrowRight', bubbles: true }),
		);
		assert(service.activeTab?.id === '1', 'Arrow navigation failed');
		service.move('1', 0);
		assert(service.tabs[0]?.id === '1', 'Reorder failed');
		await service.close('1');
		assert(service.activeTab?.id !== '1', 'Closed tab is still active');
		assert(errors.length === 0, 'Unexpected tab errors');
		return true;
	} finally {
		finish?.();
		tabs.dispose();
		host.dispose();
		service.dispose();
		changed.dispose();
		fixture.remove();
	}
}
