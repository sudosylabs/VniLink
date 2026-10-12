import { Emitter } from '../../../../base/common/event.js';
import { Disposable, DisposableStore, dispose } from '../../../../base/common/lifecycle.js';
import { registerSingleton, InstantiationType } from '../../../../platform/instantiation/common/extensions.js';
import { ITabService, type WorkbenchTab, type TabAction } from './tabService.js';

export class TabService extends Disposable implements ITabService {
	declare public readonly _serviceBrand: undefined;
	private readonly changed = this._register(new Emitter<void>());
	public readonly onDidChange = this.changed.event;
	private readonly inputs: WorkbenchTab[] = [];
	private selected: WorkbenchTab | undefined;
	private readonly listeners = new Map<WorkbenchTab, DisposableStore>();
	private readonly running = new Map<WorkbenchTab, Map<string, AbortController>>();
	private readonly closing = new Map<string, Promise<boolean>>();
	public get tabs(): readonly WorkbenchTab[] {
		return this.inputs.slice();
	}
	public get activeTab(): WorkbenchTab | undefined {
		return this.selected;
	}
	public open(tab: WorkbenchTab): void {
		if (this._store.isDisposed) {
			tab.dispose();
			throw new Error('Tab service disposed');
		}
		const existing = this.inputs.find((input) => input.id === tab.id);
		if (existing) {
			if (existing !== tab) {
				tab.dispose();
			}
			this.select(existing.id);
			return;
		}
		if (!tab.id || new Set(tab.actions.map((action) => action.id)).size !== tab.actions.length) {
			tab.dispose();
			throw new Error('Invalid tab identity or action IDs');
		}
		this.inputs.push(tab);
		const store = new DisposableStore();
		this.listeners.set(tab, store);
		if (tab.onDidChangeActions) {
			store.add(
				tab.onDidChangeActions(() => {
					this.changed.fire();
				}),
			);
		}
		this.selected = tab;
		this.changed.fire();
	}
	public select(id: string): void {
		const tab = this.inputs.find((input) => input.id === id);
		if (tab && tab !== this.selected) {
			this.selected = tab;
			this.changed.fire();
		}
	}
	public move(id: string, index: number): void {
		const previous = this.inputs.findIndex((input) => input.id === id);
		if (previous < 0 || !Number.isInteger(index)) {
			return;
		}
		const tab = this.inputs.splice(previous, 1)[0];
		this.inputs.splice(Math.max(0, Math.min(index, this.inputs.length)), 0, tab);
		this.changed.fire();
	}
	public close(id: string): Promise<boolean> {
		const pending = this.closing.get(id);
		if (pending) {
			return pending;
		}
		const tab = this.inputs.find((input) => input.id === id);
		if (!tab) {
			return Promise.resolve(false);
		}
		const operation = Promise.resolve()
			.then(async () => {
				if (tab.canClose && !(await tab.canClose())) {
					return false;
				}
				if (this._store.isDisposed) {
					return false;
				}
				const index = this.inputs.indexOf(tab);
				if (index < 0) {
					return false;
				}
				this.inputs.splice(index, 1);
				if (this.selected === tab) {
					this.selected = this.inputs[Math.max(0, index - 1)];
				}
				this.listeners.get(tab)?.dispose();
				this.listeners.delete(tab);
				for (const controller of this.running.get(tab)?.values() ?? []) {
					controller.abort();
				}
				this.running.delete(tab);
				try {
					tab.dispose();
				} finally {
					this.changed.fire();
				}
				return true;
			})
			.finally(() => {
				this.closing.delete(id);
				if (!this._store.isDisposed) {
					this.changed.fire();
				}
			});
		this.closing.set(id, operation);
		this.changed.fire();
		return operation;
	}
	public isActionEnabled(tab: WorkbenchTab, action: TabAction): boolean {
		return (
			!this._store.isDisposed &&
			this.inputs.includes(tab) &&
			tab.actions.some((candidate) => candidate.id === action.id && candidate.enabled !== false) &&
			action.enabled !== false &&
			!this.running.get(tab)?.has(action.id) &&
			!this.closing.has(tab.id)
		);
	}
	public async executeAction(id: string): Promise<void> {
		const tab = this.selected;
		const action = tab?.actions.find((candidate) => candidate.id === id);
		if (!tab || !action || !this.isActionEnabled(tab, action)) {
			return;
		}
		const controller = new AbortController();
		const pending = this.running.get(tab) ?? new Map<string, AbortController>();
		pending.set(action.id, controller);
		this.running.set(tab, pending);
		this.changed.fire();
		try {
			await action.run(controller.signal);
		} finally {
			if (this.running.get(tab)?.get(action.id) === controller) {
				this.running.get(tab)?.delete(action.id);
				if (!this.running.get(tab)?.size) {
					this.running.delete(tab);
				}
			}
			if (!this._store.isDisposed) {
				this.changed.fire();
			}
		}
	}
	public override dispose(): void {
		if (this._store.isDisposed) {
			return;
		}
		super.dispose();
		for (const pending of this.running.values()) {
			for (const controller of pending.values()) {
				controller.abort();
			}
		}
		this.running.clear();
		for (const store of this.listeners.values()) {
			store.dispose();
		}
		this.listeners.clear();
		const tabs = this.inputs.splice(0);
		this.selected = undefined;
		dispose(tabs);
	}
}
registerSingleton(ITabService, TabService, InstantiationType.Delayed);
