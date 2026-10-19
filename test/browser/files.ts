import { Emitter } from '../../src/base/common/event.js';
import { DisposableStore } from '../../src/base/common/lifecycle.js';
import type { IAction } from '../../src/base/common/actions.js';
import { IContextMenuService } from '../../src/platform/contextview/browser/contextView.js';
import type { FileConflictChoice, FileEntry, FileJob, FilePlaces } from '../../src/platform/engine/common/files.js';
import { InstantiationService } from '../../src/platform/instantiation/common/instantiationService.js';
import { ServiceCollection } from '../../src/platform/instantiation/common/serviceCollection.js';
import type { FilesPane, FolderLocation, UIStateKey } from '../../src/platform/state/common/uiState.js';
import '../../src/workbench/contrib/files/files.contribution.js';
import { OpenFilesAction } from '../../src/workbench/contrib/files/filesActions.js';
import { FilesView } from '../../src/workbench/contrib/files/filesView.js';
import { IDeviceService, type DeviceSummary } from '../../src/workbench/services/devices/deviceService.js';
import { IDialogService, type IDialogOptions, type IDialogResult } from '../../src/workbench/services/dialogs/dialogService.js';
import { IFilesService, type FilesFailure, type FilesItem, type FilesListing, type FilesRoute, type IFilesDisk, type IFilesSession } from '../../src/workbench/services/files/filesService.js';
import { ITabService } from '../../src/workbench/services/tabs/tabService.js';
import { TabService } from '../../src/workbench/services/tabs/tabServiceImpl.js';
import { IUIStateService } from '../../src/workbench/services/uiState/uiStateService.js';

function assert(condition: boolean, message: string): asserts condition {
	if (!condition) {
		throw new Error(message);
	}
}

async function until(condition: () => boolean, message: string): Promise<void> {
	const deadline = performance.now() + 3000;
	while (!condition()) {
		if (performance.now() > deadline) {
			throw new Error(message);
		}
		await new Promise(resolve => setTimeout(resolve, 10));
	}
}

const token = { dev: '1', ino: '1' } as const;

function entry(name: string, kind: 'file' | 'dir' = 'file', size = 0): FileEntry {
	return { name, kind, size: String(size), mtimeMs: String(Date.UTC(2026, 9, 1)), token };
}

/** A disk held in memory: folders by `root:path`. */
class MemoryDisk implements IFilesDisk {
	public readonly lists: string[] = [];
	public readonly renames: [string, string][] = [];
	public readonly deletes: string[] = [];

	constructor(private readonly placeList: FilePlaces, private readonly folders: Map<string, FileEntry[]>) { }

	public places(): Promise<FilePlaces> {
		return Promise.resolve(this.placeList);
	}

	public list(folder: FolderLocation): Promise<FilesListing> {
		const key = `${folder.root}:${folder.path}`;
		this.lists.push(key);
		const entries = this.folders.get(key);
		return entries ? Promise.resolve({ entries: [...entries], truncated: false, skipped: 0 }) : Promise.reject(new Error('InvalidSetting: the path is not a directory'));
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

	public delete(folder: FolderLocation, item: FileEntry): Promise<void> {
		this.deletes.push(item.name);
		const entries = this.folders.get(`${folder.root}:${folder.path}`) ?? [];
		entries.splice(entries.findIndex(candidate => candidate.name === item.name), 1);
		return Promise.resolve();
	}
}

const job = (id: string, state: FileJob['state'], extra: Partial<FileJob> = {}): FileJob => ({
	id, peer: 'studio', label: 'vnilink-demo.mov', direction: 'Upload', sender: true, state,
	total_bytes: '1000000', done_bytes: '250000', files_total: '3', files_done: '0', resumed_files: '0', skipped_files: '0', skipped_symlinks: '0', waiting_file: null, error: null, ...extra,
});

class FakeSession implements IFilesSession {
	public readonly deviceId = 'studio';
	private readonly changed = new Emitter<void>();
	public readonly onDidChangeJobs = this.changed.event;
	private readonly endedEmitter = new Emitter<void>();
	public readonly onDidEnd = this.endedEmitter.event;
	public jobs: FileJob[] = [];
	public ended = false;
	public released = 0;
	public readonly copies: { from: FilesPane; items: readonly FilesItem[]; to: FolderLocation }[] = [];
	public readonly resolutions: [string, FileConflictChoice, boolean][] = [];

	constructor(public readonly local: IFilesDisk, public readonly remote: IFilesDisk) { }

	public routeOf(): FilesRoute | undefined {
		return { from: 'local', source: { root: '0', path: 'Users/hammed/Downloads' }, destination: { root: '2', path: 'Users/abass/Downloads' } };
	}

	public copy(from: FilesPane, items: readonly FilesItem[], to: FolderLocation): Promise<void> {
		this.copies.push({ from, items, to });
		this.set([...this.jobs, job(String(this.jobs.length + 10), 'Transferring')]);
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

function press(target: Element, key: string, keyCode: number, shiftKey = false): void {
	target.dispatchEvent(new KeyboardEvent('keydown', { key, keyCode, shiftKey, bubbles: true, cancelable: true }));
}

/** The Files tab's services over two in-memory disks, and a host it fills. */
function filesFixture(width: number, height: number) {
	const disposables = new DisposableStore();
	const device: DeviceSummary = { id: 'studio', name: 'Sudosy', os: 'windows', presence: 'connected', hostname: 'sudosy', direct: 1, relays: 0 };
	const localDisk = new MemoryDisk({ roots: [{ id: '0', name: '/' }], folders: [{ kind: 'home', root: '0', path: 'Users/hammed' }, { kind: 'downloads', root: '0', path: 'Users/hammed/Downloads' }] }, new Map([
		['0:Users/hammed', [entry('Downloads', 'dir'), entry('Documents', 'dir'), entry('.zshrc', 'file', 40)]],
		['0:Users/hammed/Downloads', [entry('vnilink-demo.mov', 'file', 412_000_000), entry('invoice.pdf', 'file', 184_000), entry('Screenshots', 'dir'), entry('notes.md', 'file', 4000)]],
		['0:Users/hammed/Downloads/Screenshots', []],
		['0:Users/hammed/Documents', []],
	]));
	const remoteDisk = new MemoryDisk({ roots: [{ id: '2', name: 'C:' }, { id: '3', name: 'D:' }], folders: [{ kind: 'home', root: '2', path: 'Users/abass' }] }, new Map([
		['2:Users/abass', [entry('Downloads', 'dir'), entry('Desktop', 'dir')]],
		['2:Users/abass/Downloads', [entry('report-q3.xlsx', 'file', 2_300_000), entry('Installers', 'dir')]],
		['2:Users/abass/Downloads/Installers', []],
		['2:', [entry('Users', 'dir')]],
	]));
	const session = new FakeSession(localDisk, remoteDisk);
	const openings: (FilesFailure | undefined)[] = [];
	const state = new Map<UIStateKey, unknown>([['files.lastFolder.studio.local', { root: '0', path: 'Users/hammed/Downloads' }]]);
	const dialogs: IDialogOptions[] = [];
	const answers: IDialogResult[] = [];
	const noActions: readonly IAction[] = [];
	const shown = { menu: noActions };
	const tabService = disposables.add(new TabService());
	const services = disposables.add(new InstantiationService(new ServiceCollection(
		[ITabService, tabService],
		[IDeviceService, { _serviceBrand: undefined, devices: [device] }],
		[IFilesService, {
			_serviceBrand: undefined,
			local: localDisk,
			open: () => {
				const failure = openings.shift();
				return Promise.resolve(failure ? { failure } : { session });
			},
		}],
		[IUIStateService, {
			_serviceBrand: undefined,
			get: (key: UIStateKey) => Promise.resolve(state.get(key)),
			set: (key: UIStateKey, value: unknown) => {
				state.set(key, value);
				return Promise.resolve();
			},
		}],
		[IDialogService, {
			_serviceBrand: undefined,
			show: (options: IDialogOptions) => {
				dialogs.push(options);
				return Promise.resolve(answers.shift() ?? { button: options.cancelId, checked: false });
			},
		}],
		[IContextMenuService, {
			_serviceBrand: undefined,
			onDidShowContextMenu: new Emitter<void>().event,
			onDidHideContextMenu: new Emitter<void>().event,
			showContextMenu: (delegate: { getActions(): readonly IAction[] }) => {
				shown.menu = delegate.getActions();
			},
		}],
	), true));
	const host = document.createElement('div');
	host.className = 'view-host';
	host.style.cssText = `position:fixed;left:0;top:40px;width:${String(Math.min(width, window.innerWidth))}px;height:${String(Math.min(height, window.innerHeight - 40))}px;z-index:99;display:flex;`;
	document.body.append(host);
	return { disposables, device, localDisk, session, openings, state, dialogs, answers, shown, tabService, services, host };
}

export async function verifyFiles(): Promise<void> {
	const { disposables, device, localDisk, session, openings, state, dialogs, answers, shown, tabService, services, host } = filesFixture(1280, 720);
	let menu: readonly IAction[] = [];
	const panes = (view: FilesView): HTMLElement[] => [...view.element.querySelectorAll<HTMLElement>('.files-pane')];
	const names = (pane: HTMLElement): string[] => [...pane.querySelectorAll('.monaco-list-row .files-row-name')].map(node => node.textContent);
	const crumbs = (pane: HTMLElement): string[] => [...pane.querySelectorAll('.files-crumb')].map(node => node.textContent);
	const list = (pane: HTMLElement): HTMLElement => {
		const node = pane.querySelector<HTMLElement>('.monaco-list');
		assert(node !== null, 'A pane has no list');
		return node;
	};
	const run = (id: string): void => {
		menu = shown.menu;
		const action = menu.find(candidate => candidate.id === id);
		assert(action !== undefined, `The menu has no ${id}: ${menu.map(candidate => candidate.id).join(', ')}`);
		void action.run();
	};
	try {
		services.invokeFunction(accessor => {
			new OpenFilesAction().run(accessor, device.id);
		});
		const view = tabService.activeTab;
		assert(view instanceof FilesView && view.id === 'files:studio' && view.label === 'Sudosy', 'Files did not open a tab for the computer');
		host.append(view.element);
		const [local, remote] = panes(view);
		// This computer reopens at its last folder; the other, never opened, at home.
		await until(() => names(local).includes('vnilink-demo.mov'), 'This computer did not reopen its last folder');
		await until(() => names(remote).includes('Downloads'), 'The other computer did not open at home');
		assert(crumbs(local).join('/') === 'Users/hammed/Downloads' && crumbs(remote).join('/') === 'C:/Users/abass', `Wrong locations: ${crumbs(local).join('/')} | ${crumbs(remote).join('/')}`);
		// Folders first, natural order; dotfiles hidden until asked for.
		assert(names(local).join(',') === 'Screenshots,invoice.pdf,notes.md,vnilink-demo.mov', `Wrong order: ${names(local).join(',')}`);
		const status = view.element.querySelector('.files-status-selection')?.textContent ?? '';
		assert(/^This (Mac|PC|Computer): 4 items$/.test(status), `Wrong status: ${status}`);
		const copyButton = view.element.querySelector<HTMLButtonElement>('.files-copy-button');
		assert(copyButton !== null, 'There is no copy button');
		assert(copyButton.disabled, 'Copy is offered with nothing selected');

		// The places menu goes to the other computer's drives.
		remote.querySelector<HTMLButtonElement>('.files-computer')?.click();
		menu = shown.menu;
		assert(menu.map(action => action.label).join(',') === 'Home,,C:,D:', `Wrong places: ${menu.map(action => action.label).join(',')}`);
		run('files.root.2');
		await until(() => crumbs(remote).join('/') === 'C:', 'The drive did not open');
		remote.querySelector<HTMLButtonElement>('.files-icon-button[aria-label="Back"]')?.click();
		await until(() => crumbs(remote).join('/') === 'C:/Users/abass', 'Back did not return home');
		list(remote).focus();
		press(list(remote), 'ArrowDown', 40);
		press(list(remote), 'Enter', 13);
		await until(() => crumbs(remote).join('/') === 'C:/Users/abass/Downloads', 'Enter did not open the folder');
		assert(JSON.stringify(state.get('files.lastFolder.studio.remote')) === JSON.stringify({ root: '2', path: 'Users/abass/Downloads' }), 'The other computer\'s folder was not remembered');

		// Arrows select, Shift extends; the copy button names what and where.
		list(local).focus();
		press(list(local), 'ArrowDown', 40);
		press(list(local), 'ArrowDown', 40, true);
		await until(() => !copyButton.disabled, 'Selecting did not offer copy');
		assert(copyButton.getAttribute('aria-label') === 'Copy 2 to Downloads on Sudosy', `Wrong copy label: ${copyButton.getAttribute('aria-label') ?? ''}`);
		assert(local.classList.contains('active') && !remote.classList.contains('active'), 'The pane being worked in is not marked');
		copyButton.click();
		await until(() => session.copies.length === 1, 'Copy did not start');
		assert(session.copies[0].from === 'local' && session.copies[0].items.map(item => item.name).join(',') === 'invoice.pdf,notes.md' && session.copies[0].to.path === 'Users/abass/Downloads', `Wrong copy: ${JSON.stringify(session.copies[0])}`);
		const panel = view.element.querySelector<HTMLElement>('.files-transfers');
		assert(panel !== null && !panel.hidden, 'Copying did not show the transfers');

		// Transfers say where each goes and what fits it; a clash asks once for the rest.
		session.set([job('20', 'Transferring'), job('21', 'WaitingDecision', { direction: 'Download', waiting_file: 'report-q3.xlsx' }), job('22', 'Complete', { done_bytes: '1000000' })]);
		const rows = [...view.element.querySelectorAll('.files-transfer')];
		assert(rows.length === 3, `Wrong transfer rows: ${String(rows.length)}`);
		assert(rows[0].querySelector('.files-transfer-route')?.textContent === 'Downloads on This Mac → Downloads on Sudosy' || (rows[0].querySelector('.files-transfer-route')?.textContent ?? '').endsWith('→ Downloads on Sudosy'), `Wrong route: ${rows[0].querySelector('.files-transfer-route')?.textContent ?? ''}`);
		assert(rows[1].querySelector('.files-transfer-status')?.textContent === '“report-q3.xlsx” is already there', `Wrong waiting text: ${rows[1].querySelector('.files-transfer-status')?.textContent ?? ''}`);
		assert(view.element.querySelector('.files-status-decision')?.textContent === '1 needs your decision', 'The status bar does not ask for the decision');
		answers.push({ button: 2, checked: true });
		rows[1].querySelector<HTMLButtonElement>('.files-transfer-action button')?.click();
		await until(() => session.resolutions.length === 1, 'Choosing did not resolve');
		assert(JSON.stringify(session.resolutions[0]) === JSON.stringify(['21', 'keepBoth', true]), `Wrong resolution: ${JSON.stringify(session.resolutions[0])}`);
		assert(dialogs.at(-1)?.checkbox?.label === 'Same answer for the other names that clash', 'The conflict did not offer the same answer for the rest');
		assert(rows[2].querySelector('.files-transfer-action button') === null, 'A finished transfer offers an action');

		// Rename in place: a taken name is refused and said, the item kept.
		press(list(local), 'ArrowUp', 38);
		press(list(local), 'F2', 113);
		await until(() => local.querySelector('.files-row-name input') !== null, 'Rename did not open in the row');
		const input = local.querySelector<HTMLInputElement>('.files-row-name input');
		assert(input !== null && input.value === 'invoice.pdf' && input.selectionEnd === 'invoice'.length, 'Rename did not select the name without its extension');
		input.value = 'notes.md';
		input.dispatchEvent(new Event('input'));
		input.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
		await until(() => !(local.querySelector<HTMLElement>('.files-notice')?.hidden ?? true), 'A taken name was not reported');
		assert(local.querySelector('.files-notice')?.textContent === 'That name is already taken here.' && names(local).includes('invoice.pdf'), 'A refused rename lost the item or its reason');

		// Delete asks first, says it is permanent, and Cancel keeps everything.
		list(local).focus();
		press(list(local), 'Delete', 46);
		await until(() => dialogs.length === 2, 'Delete did not ask');
		assert(dialogs[1].message === 'Permanently delete “invoice.pdf” from This Mac?' || dialogs[1].message.startsWith('Permanently delete “invoice.pdf” from'), `Wrong delete question: ${dialogs[1].message}`);
		assert(localDisk.deletes.length === 0, 'Delete ran without confirmation');
		answers.push({ button: 0, checked: false });
		press(list(local), 'Delete', 46);
		await until(() => localDisk.deletes.join() === 'invoice.pdf', 'Confirmed delete did not run');
		await until(() => !names(local).includes('invoice.pdf'), 'The deleted item stayed listed');

		// Hidden files, from the pane's menu, are remembered.
		local.querySelector<HTMLButtonElement>('.files-icon-button[aria-label="More Actions"]')?.click();
		run('files.hidden');
		assert(state.get('files.showHidden') === true, 'Show Hidden Files was not remembered');

		// The other computer turned Files off, then the connection ends.
		session.end();
		await until(() => (remote.querySelector('.files-state-title')?.textContent ?? '') === 'Not connected to Sudosy.', 'A lost connection did not show in the pane');
		openings.push('turnedOff');
		remote.querySelector<HTMLButtonElement>('.files-state-action')?.click();
		await until(() => (remote.querySelector('.files-state-title')?.textContent ?? '') === 'Sudosy doesn’t share its files.', 'Files turned off there is not said');
		assert(names(local).length > 0, 'This computer\'s pane stopped working with the other');
		assert(await tabService.close(view.id), 'The tab did not close');
		assert(session.released >= 1, 'Closing the tab did not let go of the connection');
	} finally {
		host.remove();
		disposables.dispose();
	}
}

/**
 * The Files tab as people see it mid-task, for screenshots: Downloads on both
 * computers, two items selected here, a transfer running and one waiting.
 */
export async function showFilesFixture(width: number, height: number): Promise<{ dispose(): void }> {
	const { disposables, device, session, tabService, services, host } = filesFixture(width, height);
	services.invokeFunction(accessor => {
		new OpenFilesAction().run(accessor, device.id);
	});
	const view = tabService.activeTab;
	assert(view instanceof FilesView, 'Files did not open');
	host.append(view.element);
	const [local, remote] = [...view.element.querySelectorAll<HTMLElement>('.files-pane')];
	await until(() => local.querySelectorAll('.monaco-list-row').length > 0 && remote.querySelectorAll('.monaco-list-row').length > 0, 'The panes did not list');
	const remoteList = remote.querySelector<HTMLElement>('.monaco-list');
	remoteList?.focus();
	press(remoteList ?? remote, 'ArrowDown', 40);
	press(remoteList ?? remote, 'Enter', 13);
	await until(() => [...remote.querySelectorAll('.files-crumb')].at(-1)?.textContent === 'Downloads', 'The other computer did not open Downloads');
	const localList = local.querySelector<HTMLElement>('.monaco-list');
	localList?.focus();
	press(localList ?? local, 'ArrowDown', 40);
	press(localList ?? local, 'ArrowDown', 40, true);
	session.set([job('20', 'Transferring', { done_bytes: '412000', total_bytes: '1500000' }), job('21', 'WaitingDecision', { direction: 'Download', waiting_file: 'report-q3.xlsx', label: 'report-q3.xlsx' }), job('22', 'Complete', { label: 'Screenshots', done_bytes: '1000000' })]);
	view.element.querySelector<HTMLButtonElement>('.files-status-transfers')?.click();
	return {
		dispose: () => {
			host.remove();
			disposables.dispose();
		},
	};
}
