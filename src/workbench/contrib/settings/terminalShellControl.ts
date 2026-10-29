import * as nls from '../../../nls.js';
import { addDisposableListener, EventType } from '../../../base/browser/dom.js';
import { Emitter } from '../../../base/common/event.js';
import { Disposable } from '../../../base/common/lifecycle.js';
import type { ISettingSchema } from '../../../platform/configuration/common/configurationSchema.js';
import type { TerminalShells } from '../../../platform/engine/common/engine.js';
import { ITerminalHostService } from '../../services/terminal/terminalHostService.js';
import { createSelect, renderSettingRow, wrapSelect, type ISettingControl } from './settingRenderers.js';

/**
 * `terminal.shell`: the shells installed on this computer, after "Automatic",
 * which names what it resolves to here. The list is read again whenever the
 * page is shown, so a shell installed meanwhile appears.
 */
export class TerminalShellControl extends Disposable implements ISettingControl {
	private readonly changed = this._register(new Emitter<unknown>());
	public readonly onDidChange = this.changed.event;
	private readonly select = createSelect([['', nls.localize('terminal.shellAutomatic', 'Automatic')]]);
	private help: HTMLElement | undefined;
	private shells: TerminalShells | undefined;
	private value: unknown = '';
	private enabled = false;
	private loading: Promise<void> | undefined;
	private renderedOptions = '';

	constructor(
		private readonly key: string,
		private readonly schema: ISettingSchema,
		@ITerminalHostService private readonly terminalHost: ITerminalHostService,
	) {
		super();
	}

	public render(container: HTMLElement): void {
		const row = renderSettingRow(container, this.schema.title ?? this.key, this.schema.description, this.select);
		this.help = row.help;
		this.select.dataset.setting = this.key;
		row.control.append(wrapSelect(this.select));
		this._register(addDisposableListener(this.select, EventType.CHANGE, () => {
			this.changed.fire(this.select.value);
		}));
		void this.refresh();
	}

	public refresh(): Promise<void> {
		this.loading ??= this.load().finally(() => {
			this.loading = undefined;
		});
		return this.loading;
	}

	private async load(): Promise<void> {
		try {
			const shells = await this.terminalHost.shells();
			if (this._store.isDisposed) {
				return;
			}
			this.shells = shells;
			this.renderOptions();
		} catch {
			if (!this._store.isDisposed) {
				this.setMessage(nls.localize('terminal.shellsFailed', 'The shells on this computer could not be checked.'));
			}
		} finally {
			if (!this._store.isDisposed) {
				this.setEnabled(this.enabled);
			}
		}
	}

	private renderOptions(): void {
		const shells = this.shells;
		if (!shells) {
			return;
		}
		const automatic = shells.shells.find(shell => shell.id === shells.automatic)?.label ?? shells.automatic;
		const options: [string, string][] = [['', nls.localize('terminal.shellAutomaticNamed', 'Automatic ({0})', automatic)]];
		for (const shell of shells.shells) {
			options.push([shell.id, shell.label]);
		}
		// A saved shell that was uninstalled stays visible and chosen, so it returns when reinstalled.
		const saved = typeof this.value === 'string' ? this.value : '';
		const missing = saved !== '' && !shells.shells.some(shell => shell.id === saved);
		if (missing) {
			options.push([saved, nls.localize('terminal.shellMissing', '{0} (not installed)', saved)]);
		}
		// Rebuilding the options closes an open list, so only a different list is rebuilt.
		const signature = JSON.stringify(options);
		if (signature !== this.renderedOptions) {
			this.renderedOptions = signature;
			this.select.replaceChildren(...Array.from(createSelect(options).options));
		}
		if (this.select.value !== saved) {
			this.select.value = saved;
		}
		this.setMessage(missing ? nls.localize('terminal.shellMissingHelp', 'Not installed on this computer; paired computers get the automatic shell.') : '');
	}

	/** A problem replaces the help until it is resolved. */
	private setMessage(message: string): void {
		if (this.help) {
			this.help.textContent = message || (this.schema.description ?? '');
		}
	}

	public setValue(value: unknown): void {
		this.value = value;
		if (this.shells) {
			this.renderOptions();
		} else {
			this.select.value = typeof value === 'string' ? value : '';
		}
	}

	public setEnabled(enabled: boolean): void {
		this.enabled = enabled;
		this.select.disabled = !enabled || !this.shells;
	}
}
