import { Emitter } from '../../../base/common/event.js';
import { Disposable, dispose } from '../../../base/common/lifecycle.js';
import { registerSingleton, InstantiationType } from '../../../platform/instantiation/common/extensions.js';
import { ITabService, type WorkbenchTab } from './tabService.js';

export class TabService extends Disposable implements ITabService {
	declare public readonly _serviceBrand: undefined;
	private readonly changed = this._register(new Emitter<void>());
	public readonly onDidChange = this.changed.event;
	private readonly inputs: WorkbenchTab[] = [];
	private selected: WorkbenchTab | undefined;
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
		if (!tab.id) {
			tab.dispose();
			throw new Error('Invalid tab identity');
		}
		if (tab.permanent) {
			this.inputs.splice(this.inputs.filter(input => input.permanent).length, 0, tab);
		} else {
			this.inputs.push(tab);
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
		if (previous < 0 || !Number.isInteger(index) || this.inputs[previous].permanent) {
			return;
		}
		const tab = this.inputs.splice(previous, 1)[0];
		const firstMovable = this.inputs.filter(input => input.permanent).length;
		this.inputs.splice(Math.max(firstMovable, Math.min(index, this.inputs.length)), 0, tab);
		this.changed.fire();
	}

	public close(id: string): Promise<boolean> {
		const pending = this.closing.get(id);
		if (pending) {
			return pending;
		}
		const tab = this.inputs.find((input) => input.id === id);
		if (!tab || tab.permanent) {
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

	public override dispose(): void {
		if (this._store.isDisposed) {
			return;
		}
		super.dispose();
		const tabs = this.inputs.splice(0);
		this.selected = undefined;
		dispose(tabs);
	}
}

registerSingleton(ITabService, TabService, InstantiationType.Delayed);
