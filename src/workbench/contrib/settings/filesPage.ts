import * as nls from '../../../nls.js';
import { Disposable, DisposableStore } from '../../../base/common/lifecycle.js';
import { addDisposableListener } from '../../../base/browser/domEvent.js';
import { renderIcon } from '../../../base/browser/ui/icon/icon.js';
import { Icons } from '../../../base/common/icons.js';
import { IConfigurationSnapshotService, isFileRoots } from '../../../platform/configuration/common/configurationSnapshot.js';
import { createPageHeading, type IHomePage } from '../home/homePages.js';
import './settingsControls.js';
import './media/files.css';

interface FolderRow { readonly input: HTMLInputElement; readonly element: HTMLElement; readonly store: DisposableStore; }

export class FilesPage extends Disposable implements IHomePage {
	public readonly element = document.createElement('main');
	private readonly fields = document.createElement('fieldset');
	private readonly list = document.createElement('div');
	private readonly add = document.createElement('button');
	private readonly save = document.createElement('button');
	private readonly status = document.createElement('p');
	private readonly empty = document.createElement('p');
	private readonly rows: FolderRow[] = [];
	private baseline = this.configuration.snapshot.values['files.roots'];
	private busy = false;

	constructor(@IConfigurationSnapshotService private readonly configuration: IConfigurationSnapshotService) {
		super();
		this.element.className = 'files-page';
		this.element.dataset.settingsPage = 'files';
		this.element.append(createPageHeading(nls.localize('home.files', 'Files'), nls.localize('files.description', 'Manage optional restrictions on remote file access.')));
		const content = document.createElement('div');
		content.className = 'files-content';
		const form = document.createElement('form');
		form.noValidate = true;
		this.fields.className = 'files-fields';
		const heading = document.createElement('legend');
		heading.textContent = nls.localize('files.shared', 'Folder restrictions');
		const help = document.createElement('p');
		help.className = 'settings-help';
		help.textContent = nls.localize('files.help', 'By default, permitted devices can browse the filesystem within this app’s operating-system access. Add folders only to restrict browsing. Devices still need Files permission.');
		this.list.className = 'files-folder-list';
		this.empty.className = 'files-empty';
		this.empty.append(renderIcon(Icons.files));
		const emptyText = document.createElement('span');
		emptyText.textContent = nls.localize('files.empty', 'No folder restrictions');
		this.empty.append(emptyText);
		this.add.type = 'button';
		this.add.className = 'settings-button';
		this.add.textContent = nls.localize('files.add', 'Add folder');
		const format = document.createElement('p');
		format.className = 'settings-help';
		format.textContent = nls.localize('files.pathHelp', 'Enter the full path to an existing folder on this computer. Up to 16 folders. Changes require incoming connections to be closed.');
		this.fields.append(heading, help, this.list, this.empty, this.add, format);
		const footer = document.createElement('div');
		footer.className = 'settings-apply-row';
		this.save.type = 'submit';
		this.save.className = 'settings-button primary';
		this.save.textContent = nls.localize('files.save', 'Save changes');
		footer.append(this.save);
		this.status.className = 'settings-status';
		this.status.setAttribute('role', 'status');
		form.append(this.fields, footer, this.status);
		content.append(form);
		this.element.append(content);
		this.render();
		this.update();
		this._register(addDisposableListener(this.add, 'click', () => {
			if (!this.busy && this.rows.length < 16) {
				this.addRow('');
				this.update();
				this.rows.at(-1)?.input.focus();
			}
		}));
		this._register(addDisposableListener(form, 'input', () => {
			this.update();
		}));
		this._register(addDisposableListener(form, 'submit', event => {
			event.preventDefault();
			void this.saveFolders();
		}));
		this._register(configuration.onDidChangeConfiguration(() => {
			const clean = !this.dirty();
			this.baseline = configuration.snapshot.values['files.roots'];
			if (!this.busy && clean) {
				this.render();
			}
			this.update();
		}));
	}

	private addRow(path: string): void {
		const store = new DisposableStore();
		const element = document.createElement('div');
		element.className = 'files-folder-row';
		const input = document.createElement('input');
		input.type = 'text';
		input.value = path;
		input.placeholder = nls.localize('files.path', 'Full folder path');
		input.setAttribute('aria-label', nls.localize('files.folderPath', 'Shared folder path'));
		input.spellcheck = false;
		const remove = document.createElement('button');
		remove.type = 'button';
		remove.className = 'settings-text-button';
		remove.setAttribute('aria-label', nls.localize('files.remove', 'Remove folder'));
		remove.append(renderIcon(Icons.close));
		const row = { input, element, store };
		element.append(renderIcon(Icons.files), input, remove);
		this.list.append(element);
		this.rows.push(row);
		store.add(addDisposableListener(remove, 'click', () => {
			if (!this.busy) {
				this.rows.splice(this.rows.indexOf(row), 1);
				store.dispose();
				element.remove();
				this.update();
			}
		}));
	}

	private render(): void {
		for (const row of this.rows.splice(0)) {
			row.store.dispose();
		}
		this.list.replaceChildren();
		for (const path of this.baseline) {
			this.addRow(path);
		}
	}

	private draft(): string[] {
		return this.rows.map(row => row.input.value.trim());
	}

	private dirty(): boolean {
		return JSON.stringify(this.draft()) !== JSON.stringify(this.baseline);
	}

	private update(): void {
		this.fields.disabled = this.busy || !!this.configuration.snapshot.error;
		this.save.disabled = this.fields.disabled || !this.dirty();
		this.add.disabled = this.rows.length >= 16;
		this.empty.hidden = !!this.rows.length;
		if (this.configuration.snapshot.error) {
			this.status.textContent = this.configuration.snapshot.error;
		}
	}

	private async saveFolders(): Promise<void> {
		if (this.busy || this.save.disabled || this.disposed()) {
			return;
		}
		const paths = this.draft();
		if (!isFileRoots(paths)) {
			this.status.textContent = nls.localize('files.invalid', 'Use unique, full folder paths. Remove empty rows before saving.');
			return;
		}
		this.busy = true;
		this.update();
		this.status.textContent = '';
		try {
			await this.configuration.updateValue({ key: 'files.roots', value: paths });
			if (!this.disposed()) {
				this.baseline = this.configuration.snapshot.values['files.roots'];
				this.render();
			}
		} catch {
			if (!this.disposed()) {
				this.status.textContent = nls.localize('files.failed', 'Could not save shared folders. Check that each folder exists and close incoming connections before trying again.');
			}
		} finally {
			if (!this.disposed()) {
				this.busy = false;
				this.update();
			}
		}
	}

	private disposed(): boolean {
		return this._store.isDisposed;
	}

	public override dispose(): void {
		for (const row of this.rows) {
			row.store.dispose();
		}
		super.dispose();
		this.element.remove();
	}
}
