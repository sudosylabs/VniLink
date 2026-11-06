// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import '../../files.contribution.js';
import { $ } from '../../../../../base/browser/dom.js';
import type { IAction } from '../../../../../base/common/actions.js';
import { Emitter } from '../../../../../base/common/event.js';
import { Disposable, toDisposable, type IDisposable } from '../../../../../base/common/lifecycle.js';
import { until } from '../../../../../base/test/browser/waits.js';
import type { IContextMenuDelegate } from '../../../../../base/browser/contextmenu.js';
import { IContextMenuService, type IContextMenuMenuDelegate } from '../../../../../platform/contextview/browser/contextView.js';
import type { FileConflictChoice, FileEntry, FileJob, FilePlaces } from '../../../../../platform/engine/common/files.js';
import { TestInstantiationService } from '../../../../../platform/instantiation/test/common/instantiationServiceMock.js';
import type { FilesPane, FolderLocation, UIStateKey } from '../../../../../platform/state/common/uiState.js';
import { IDeviceService, type DeviceSummary } from '../../../../services/devices/deviceService.js';
import { TestDeviceService } from '../../../../services/devices/test/common/testDeviceService.js';
import { IDialogService, type IDialogOptions, type IDialogResult } from '../../../../services/dialogs/dialogService.js';
import { IFilesService, type FilesFailure, type FilesItem, type FilesListing, type IFilesDisk, type IFilesSession } from '../../../../services/files/filesService.js';
import { ITabService } from '../../../../services/tabs/tabService.js';
import { TabService } from '../../../../services/tabs/tabServiceImpl.js';
import { IUIStateService } from '../../../../services/uiState/uiStateService.js';
import { OpenFilesAction } from '../../filesActions.js';
import { FilesView } from '../../filesView.js';

const token = { kind: 'identity', dev: '1', ino: '1', born: '7', fingerprint: null } as const;

/** A folder entry named `name`, changed on 1 October 2026. */
export function entry(name: string, kind: 'file' | 'dir' = 'file', size = 0): FileEntry {
	return { name, kind, size: String(size), mtimeMs: String(Date.UTC(2026, 9, 1)), token };
}

/** A transfer of the computer `studio`, a quarter done unless `extra` says otherwise. */
export function job(id: string, state: FileJob['state'], extra: Partial<FileJob> = {}): FileJob {
	return {
		id, peer: 'studio', label: 'vnilink-demo.mov', direction: 'Upload', sender: true, state,
		total_bytes: '1000000', done_bytes: '250000', files_total: '3', files_done: '0', resumed_files: '0', skipped_files: '0', skipped_symlinks: '0', waiting_file: null, current_file: null, error: null, destination: null, ...extra,
	};
}

/** A disk held in memory: folders by `root:path`. Listings, renames and deletes are kept. */
export class TestFilesDisk implements IFilesDisk {
	public readonly lists: string[] = [];
	public readonly renames: [string, string][] = [];
	public readonly deletes: string[] = [];
	/** Who watches which folder, to say when it changes outside VniLink. */
	public readonly watchers = new Map<string, () => void>();
	/** Names whose place has no Trash, as a network share has none. */
	public readonly noTrash = new Set<string>();
	/** Names in each listing that are not text, so the pane can say they were left out. */
	public skipped = 0;

	constructor(private placeList: FilePlaces, private readonly folders: Map<string, FileEntry[]>) { }

	public places(): Promise<FilePlaces> {
		return Promise.resolve(this.placeList);
	}

	/** A drive plugged in: the disk names it as a volume from now on. */
	public plug(root: string, path: string): void {
		this.placeList = { ...this.placeList, folders: [...this.placeList.folders, { kind: 'volume', root, path }] };
		this.folders.set(`${root}:${path}`, []);
	}

	public list(folder: FolderLocation): Promise<FilesListing> {
		const key = `${folder.root}:${folder.path}`;
		this.lists.push(key);
		const entries = this.folders.get(key);
		return entries ? Promise.resolve({ entries: [...entries], truncated: false, skipped: this.skipped }) : Promise.reject(new Error('InvalidSetting: the path is not a directory'));
	}

	public mkdir(folder: FolderLocation, name: string): Promise<void> {
		this.folders.get(`${folder.root}:${folder.path}`)?.push(entry(name, 'dir'));
		this.folders.set(`${folder.root}:${folder.path ? `${folder.path}/` : ''}${name}`, []);
		return Promise.resolve();
	}

	public rename(folder: FolderLocation, item: FileEntry, newName: string): Promise<void> {
		const entries = this.folders.get(`${folder.root}:${folder.path}`) ?? [];
		if (entries.some(candidate => candidate.name === newName)) {
			return Promise.reject(new Error('FileExists: the name is taken'));
		}
		this.renames.push([item.name, newName]);
		const index = entries.findIndex(candidate => candidate.name === item.name);
		entries[index] = { ...item, name: newName };
		return Promise.resolve();
	}

	public watch(folder: FolderLocation, changed: () => void): IDisposable {
		const key = `${folder.root}:${folder.path}`;
		this.watchers.set(key, changed);
		return toDisposable(() => {
			if (this.watchers.get(key) === changed) {
				this.watchers.delete(key);
			}
		});
	}

	/** Removes an entry as Finder or Explorer would, and says so to whoever watches. */
	public removeOutside(folder: FolderLocation, name: string): void {
		const key = `${folder.root}:${folder.path}`;
		const entries = this.folders.get(key) ?? [];
		entries.splice(entries.findIndex(candidate => candidate.name === name), 1);
		this.watchers.get(key)?.();
	}

	/** What a transfer leaves in `folder`. */
	public land(folder: FolderLocation, item: FileEntry): void {
		this.folders.get(`${folder.root}:${folder.path}`)?.push(item);
	}

	public delete(folder: FolderLocation, item: FileEntry, trash: boolean): Promise<void> {
		if (trash && this.noTrash.has(item.name)) {
			return Promise.reject(new Error('Unsupported: the entry can’t be moved to the Trash'));
		}
		this.deletes.push(`${item.name}:${trash ? 'trash' : 'gone'}`);
		const entries = this.folders.get(`${folder.root}:${folder.path}`) ?? [];
		entries.splice(entries.findIndex(candidate => candidate.name === item.name), 1);
		return Promise.resolve();
	}
}

/** A connection to the computer `studio` whose transfers the test sets and ends. */
export class TestFilesSession extends Disposable implements IFilesSession {
	public readonly deviceId = 'studio';
	private readonly changed = this._register(new Emitter<void>());
	public readonly onDidChangeJobs = this.changed.event;
	private readonly endedEmitter = this._register(new Emitter<void>());
	public readonly onDidEnd = this.endedEmitter.event;
	public jobs: FileJob[] = [];
	public ended = false;
	public released = 0;
	public readonly copies: { from: FilesPane; items: readonly FilesItem[]; to: FolderLocation }[] = [];
	public readonly resolutions: [string, FileConflictChoice, boolean][] = [];

	constructor(public readonly local: IFilesDisk, public readonly remote: IFilesDisk) {
		super();
	}

	/** One transfer for the selection, naming each item where it lands, as the engine's does. */
	public copy(from: FilesPane, items: readonly FilesItem[], to: FolderLocation): Promise<void> {
		this.copies.push({ from, items, to });
		this.set([...this.jobs, job(String(this.jobs.length + 10), 'Transferring', {
			label: items.length > 1 ? `${items[0].name} +${String(items.length - 1)} more` : items[0].name, direction: from === 'local' ? 'Upload' : 'Download', files_total: String(items.length),
			destination: { root: to.root, path: to.path, items: items.map(item => ({ name: item.name, folder: false })) },
		})]);
		return Promise.resolve();
	}

	public set(jobs: FileJob[]): void {
		this.jobs = jobs;
		this.changed.fire();
	}

	public end(): void {
		this.ended = true;
		this.endedEmitter.fire();
	}

	public cancel(): Promise<void> {
		return Promise.resolve();
	}

	public resume(): Promise<void> {
		return Promise.resolve();
	}

	public resolve(jobId: string, choice: FileConflictChoice, applyToRemaining: boolean): Promise<void> {
		this.resolutions.push([jobId, choice, applyToRemaining]);
		return Promise.resolve();
	}

	public forget(): Promise<void> {
		return Promise.resolve();
	}

	public release(): void {
		this.released++;
	}
}

/** Presses `key` on `target`, as the keyboard does. */
export function press(target: Element, key: string, keyCode: number, shiftKey = false): void {
	target.dispatchEvent(new KeyboardEvent('keydown', { key, keyCode, shiftKey, bubbles: true, cancelable: true }));
}

/** Presses `element` as a pointer does. */
export function click(element: Element | null | undefined): void {
	element?.dispatchEvent(new MouseEvent('click', { bubbles: true, cancelable: true }));
}

/** The names a pane lists, in order. */
export function names(pane: HTMLElement): string[] {
	return [...pane.querySelectorAll('.monaco-list-row .files-row-name')].map(node => node.textContent);
}

/** The steps of a pane's path, in order. */
export function crumbs(pane: HTMLElement): string[] {
	return [...pane.querySelectorAll('.files-crumb')].map(node => node.textContent);
}

/** A pane's list. */
export function list(pane: HTMLElement): HTMLElement {
	const node = pane.querySelector<HTMLElement>('.monaco-list');
	if (!node) {
		throw new Error('A pane has no list');
	}
	return node;
}

/**
 * The Files tab of the computer `studio` over two disks held in memory, and a
 * host it fills, `width` × `height` at most. This computer reopens at its
 * last folder, Downloads; the other has never been opened. `openings`
 * answers the next connections with a failure; the dialogs asked, the
 * answers given to them (`answers`, Cancel when none is left) and the menu
 * last shown are kept.
 */
export class FilesFixture extends Disposable {
	public readonly device: DeviceSummary = { id: 'studio', name: 'Studio PC', os: 'windows', presence: 'connected', hostname: 'studio-pc', direct: 1, relays: 0 };
	public readonly localDisk = new TestFilesDisk({ roots: [{ id: '0', name: '/' }], folders: [{ kind: 'home', root: '0', path: 'Users/someone' }, { kind: 'downloads', root: '0', path: 'Users/someone/Downloads' }] }, new Map([
		['0:Users/someone', [entry('Downloads', 'dir'), entry('Documents', 'dir'), entry('.zshrc', 'file', 40)]],
		['0:Users/someone/Downloads', [entry('vnilink-demo.mov', 'file', 412_000_000), entry('invoice.pdf', 'file', 184_000), entry('Screenshots', 'dir'), entry('notes.md', 'file', 4000)]],
		['0:Users/someone/Downloads/Screenshots', []],
		['0:Users/someone/Documents', []],
	]));
	public readonly remoteDisk = new TestFilesDisk({ roots: [{ id: '2', name: 'C:' }, { id: '3', name: 'D:' }], folders: [{ kind: 'home', root: '2', path: 'Users/someone' }] }, new Map([
		['2:Users/someone', [entry('Downloads', 'dir'), entry('Desktop', 'dir')]],
		['2:Users/someone/Downloads', [entry('report-q3.xlsx', 'file', 2_300_000), entry('Installers', 'dir')]],
		['2:Users/someone/Downloads/Installers', Array.from({ length: 60 }, (_, index) => entry(`setup-${String(index).padStart(2, '0')}.exe`, 'file', 1000))],
		['2:', [entry('Users', 'dir')]],
	]));
	public readonly session = this._register(new TestFilesSession(this.localDisk, this.remoteDisk));
	public readonly openings: (FilesFailure | undefined)[] = [];
	public readonly state = new Map<UIStateKey, unknown>([['files.lastFolder.studio.local', { root: '0', path: 'Users/someone/Downloads' }]]);
	public readonly dialogs: IDialogOptions[] = [];
	public readonly answers: (IDialogResult | Promise<IDialogResult>)[] = [];
	/** The actions of the menu shown last. */
	public menu: readonly IAction[] = [];
	public readonly tabService = this._register(new TabService());
	public readonly host = $('div.view-host');
	private readonly instantiationService: TestInstantiationService;

	constructor(width = 1280, height = 720) {
		super();
		this.instantiationService = this._register(new TestInstantiationService());
		this.instantiationService.stub(ITabService, this.tabService);
		const devices = this._register(new TestDeviceService());
		devices.devices = [this.device];
		this.instantiationService.stub(IDeviceService, devices);
		this.instantiationService.stub(IFilesService, {
			local: this.localDisk,
			open: () => {
				const failure = this.openings.shift();
				return Promise.resolve(failure ? { failure } : { session: this.session });
			},
		});
		this.instantiationService.stub(IUIStateService, {
			get: (key: UIStateKey) => Promise.resolve(this.state.get(key) as never),
			set: (key: UIStateKey, value: unknown) => {
				this.state.set(key, value);
				return Promise.resolve();
			},
		});
		this.instantiationService.stub(IDialogService, {
			show: (options: IDialogOptions) => {
				this.dialogs.push(options);
				return Promise.resolve(this.answers.shift() ?? { button: options.cancelId, checked: false });
			},
		});
		this.instantiationService.stub(IContextMenuService, {
			showContextMenu: (delegate: IContextMenuDelegate | IContextMenuMenuDelegate) => {
				// The Files tab builds its menus from actions, never from a menu id.
				this.menu = delegate.getActions?.() ?? [];
			},
		});
		this.host.style.cssText = `position:fixed;left:0;top:40px;width:${String(Math.min(width, window.innerWidth))}px;height:${String(Math.min(height, window.innerHeight - 40))}px;z-index:99;display:flex;`;
		document.body.append(this.host);
	}

	/** Runs the Files row's action for the computer and mounts the tab it opens. */
	public run(): FilesView {
		this.instantiationService.invokeFunction(accessor => {
			new OpenFilesAction().run(accessor, this.device.id);
		});
		const view = this.tabService.activeTab;
		if (!(view instanceof FilesView)) {
			throw new Error('Files did not open a tab for the computer');
		}
		this.host.append(view.element);
		return view;
	}

	/** Opens the tab and waits for both panes to list: this computer's Downloads, the other's home. */
	public async open(): Promise<{ readonly view: FilesView; readonly local: HTMLElement; readonly remote: HTMLElement }> {
		const view = this.run();
		const [local, remote] = [...view.element.querySelectorAll<HTMLElement>('.files-pane')];
		await until(() => names(local).includes('vnilink-demo.mov'), 'This computer did not reopen its last folder');
		await until(() => names(remote).includes('Downloads'), 'The other computer did not open at home');
		return { view, local, remote };
	}

	/** Opens the first folder of the other computer's home, its Downloads. */
	public async openRemoteDownloads(remote: HTMLElement): Promise<void> {
		list(remote).focus();
		press(list(remote), 'ArrowDown', 40);
		press(list(remote), 'Enter', 13);
		await until(() => crumbs(remote).join('/') === 'C:/Users/someone/Downloads', 'Enter did not open the folder');
	}

	/** Selects the first two files here, the invoice and the notes, with the arrows and Shift. */
	public selectTwoHere(local: HTMLElement): void {
		list(local).focus();
		press(list(local), 'ArrowDown', 40);
		press(list(local), 'ArrowDown', 40, true);
	}

	/** Opens a pane's places menu and returns its labels; an empty label is a separator. */
	public async places(pane: HTMLElement): Promise<string[]> {
		this.menu = [];
		click(pane.querySelector('.files-computer'));
		await until(() => this.menu.length > 0, 'The places menu did not open');
		return this.menu.map(action => action.label);
	}

	/** Runs the action `id` of the menu shown last. */
	public runMenu(id: string): void {
		const action = this.menu.find(candidate => candidate.id === id);
		if (!action) {
			throw new Error(`The menu has no ${id}: ${this.menu.map(candidate => candidate.id).join(', ')}`);
		}
		void action.run();
	}

	/**
	 * The tab as people see it mid-task, for screenshots: Downloads on both
	 * computers, two items selected here, a transfer running and one waiting,
	 * and the list of transfers open.
	 */
	public async showMidTask(): Promise<void> {
		const { view, local, remote } = await this.open();
		await this.openRemoteDownloads(remote);
		this.selectTwoHere(local);
		const into = { root: '2', path: 'Users/someone/Downloads' };
		this.session.set([
			job('20', 'Transferring', { label: 'vnilink-demo.mov', files_total: '1', done_bytes: '412000', total_bytes: '1500000', destination: { ...into, items: [{ name: 'vnilink-demo.mov', folder: false }] } }),
			job('21', 'WaitingDecision', { direction: 'Download', waiting_file: 'notes.md', label: 'notes.md', files_total: '1', destination: { root: '0', path: 'Users/someone/Downloads', items: [{ name: 'notes.md', folder: false }] } }),
			job('22', 'Complete', { label: 'Screenshots', done_bytes: '1000000', destination: { ...into, items: [{ name: 'Screenshots', folder: true }] } }),
		]);
		click(view.element.querySelector('.files-status-transfers'));
	}

	public override dispose(): void {
		for (const tab of [...this.tabService.tabs]) {
			tab.dispose();
		}
		super.dispose();
		this.host.remove();
	}
}
