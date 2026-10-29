import { Emitter } from '../../src/base/common/event.js';
import { DisposableStore, toDisposable, type IDisposable } from '../../src/base/common/lifecycle.js';
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
import { IFilesService, type FilesFailure, type FilesItem, type FilesListing, type IFilesDisk, type IFilesSession } from '../../src/workbench/services/files/filesService.js';
import { ITabService } from '../../src/workbench/services/tabs/tabService.js';
import { TabService } from '../../src/workbench/services/tabs/tabServiceImpl.js';
import { IUIStateService } from '../../src/workbench/services/uiState/uiStateService.js';

function assert(condition: boolean, message: string): asserts condition {
	if (!condition) {
		throw new Error(message);
	}
}

async function until(condition: () => boolean, message: string): Promise<void> {
	const deadline = performance.now() + 5000;
	while (!condition()) {
		if (performance.now() > deadline) {
			throw new Error(message);
		}
		await new Promise(resolve => setTimeout(resolve, 10));
	}
}

const token = { kind: 'identity', dev: '1', ino: '1', born: '7', fingerprint: null } as const;

function entry(name: string, kind: 'file' | 'dir' = 'file', size = 0): FileEntry {
	return { name, kind, size: String(size), mtimeMs: String(Date.UTC(2026, 9, 1)), token };
}

/** A disk held in memory: folders by `root:path`. */
class MemoryDisk implements IFilesDisk {
	public readonly lists: string[] = [];
	public readonly renames: [string, string][] = [];
	public readonly deletes: string[] = [];

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

	/** Who watches which folder, to say when it changes outside VniLink. */
	public readonly watchers = new Map<string, () => void>();

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

	/** Names whose place has no Trash, as a network share has none. */
	public readonly noTrash = new Set<string>();

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

const job = (id: string, state: FileJob['state'], extra: Partial<FileJob> = {}): FileJob => ({
	id, peer: 'studio', label: 'vnilink-demo.mov', direction: 'Upload', sender: true, state,
	total_bytes: '1000000', done_bytes: '250000', files_total: '3', files_done: '0', resumed_files: '0', skipped_files: '0', skipped_symlinks: '0', waiting_file: null, current_file: null, error: null, destination: null, ...extra,
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

function press(target: Element, key: string, keyCode: number, shiftKey = false): void {
	target.dispatchEvent(new KeyboardEvent('keydown', { key, keyCode, shiftKey, bubbles: true, cancelable: true }));
}

/** The Files tab's services over two in-memory disks, and a host it fills. */
function filesFixture(width: number, height: number) {
	const disposables = new DisposableStore();
	const device: DeviceSummary = { id: 'studio', name: 'Studio PC', os: 'windows', presence: 'connected', hostname: 'studio-pc', direct: 1, relays: 0 };
	const localDisk = new MemoryDisk({ roots: [{ id: '0', name: '/' }], folders: [{ kind: 'home', root: '0', path: 'Users/someone' }, { kind: 'downloads', root: '0', path: 'Users/someone/Downloads' }] }, new Map([
		['0:Users/someone', [entry('Downloads', 'dir'), entry('Documents', 'dir'), entry('.zshrc', 'file', 40)]],
		['0:Users/someone/Downloads', [entry('vnilink-demo.mov', 'file', 412_000_000), entry('invoice.pdf', 'file', 184_000), entry('Screenshots', 'dir'), entry('notes.md', 'file', 4000)]],
		['0:Users/someone/Downloads/Screenshots', []],
		['0:Users/someone/Documents', []],
	]));
	const remoteDisk = new MemoryDisk({ roots: [{ id: '2', name: 'C:' }, { id: '3', name: 'D:' }], folders: [{ kind: 'home', root: '2', path: 'Users/someone' }] }, new Map([
		['2:Users/someone', [entry('Downloads', 'dir'), entry('Desktop', 'dir')]],
		['2:Users/someone/Downloads', [entry('report-q3.xlsx', 'file', 2_300_000), entry('Installers', 'dir')]],
		['2:Users/someone/Downloads/Installers', Array.from({ length: 60 }, (_, index) => entry(`setup-${String(index).padStart(2, '0')}.exe`, 'file', 1000))],
		['2:', [entry('Users', 'dir')]],
	]));
	const session = new FakeSession(localDisk, remoteDisk);
	const openings: (FilesFailure | undefined)[] = [];
	const state = new Map<UIStateKey, unknown>([['files.lastFolder.studio.local', { root: '0', path: 'Users/someone/Downloads' }]]);
	const dialogs: IDialogOptions[] = [];
	const answers: (IDialogResult | Promise<IDialogResult>)[] = [];
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
	return { disposables, device, localDisk, remoteDisk, session, openings, state, dialogs, answers, shown, tabService, services, host };
}

export async function verifyFiles(): Promise<void> {
	const fixture = filesFixture(1280, 720);
	const { disposables, device, localDisk, remoteDisk, session, openings, state } = fixture;
	const { dialogs, answers, shown, tabService, services, host } = fixture;
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
		assert(view instanceof FilesView && view.id === 'files:studio' && view.label === 'Studio PC', 'Files did not open a tab for the computer');
		host.append(view.element);
		const [local, remote] = panes(view);
		// This computer reopens at its last folder; the other, never opened, at home.
		await until(() => names(local).includes('vnilink-demo.mov'), 'This computer did not reopen its last folder');
		await until(() => names(remote).includes('Downloads'), 'The other computer did not open at home');
		assert(crumbs(local).join('/') === 'Users/someone/Downloads' && crumbs(remote).join('/') === 'C:/Users/someone', `Wrong locations: ${crumbs(local).join('/')} | ${crumbs(remote).join('/')}`);
		// Folders first, natural order; dotfiles hidden until asked for.
		assert(names(local).join(',') === 'Screenshots,invoice.pdf,notes.md,vnilink-demo.mov', `Wrong order: ${names(local).join(',')}`);
		const status = view.element.querySelector('.files-status-selection')?.textContent ?? '';
		assert(/^This (Mac|PC|Computer): 4 items$/.test(status), `Wrong status: ${status}`);
		const copyButton = view.element.querySelector<HTMLButtonElement>('.files-copy-button');
		assert(copyButton !== null, 'There is no copy button');
		assert(copyButton.disabled, 'Copy is offered with nothing selected');

		// The places menu goes to the other computer's drives.
		const placesNow = async (pane: HTMLElement): Promise<string> => {
			shown.menu = [];
			pane.querySelector<HTMLButtonElement>('.files-computer')?.click();
			await until(() => shown.menu.length > 0, 'The places menu did not open');
			menu = shown.menu;
			return menu.map(action => action.label).join(',');
		};
		const placeList = await placesNow(remote);
		assert(placeList === 'Home,,C:,D:', `Wrong places: ${placeList}`);
		// A drive plugged in after Files opened shows the next time, after the disk and drives.
		localDisk.plug('0', 'Volumes/Backup');
		const withDrive = await placesNow(local);
		assert(withDrive === 'Home,Downloads,,Whole Disk,Backup', `A drive plugged in later is missing: ${withDrive}`);
		await placesNow(remote);
		run('files.root.2');
		await until(() => crumbs(remote).join('/') === 'C:', 'The drive did not open');
		remote.querySelector<HTMLButtonElement>('.files-icon-button[aria-label="Back"]')?.click();
		await until(() => crumbs(remote).join('/') === 'C:/Users/someone', 'Back did not return home');
		list(remote).focus();
		press(list(remote), 'ArrowDown', 40);
		press(list(remote), 'Enter', 13);
		await until(() => crumbs(remote).join('/') === 'C:/Users/someone/Downloads', 'Enter did not open the folder');
		assert(JSON.stringify(state.get('files.lastFolder.studio.remote')) === JSON.stringify({ root: '2', path: 'Users/someone/Downloads' }), 'The other computer\'s folder was not remembered');

		// Arrows select, Shift extends; the copy button names what and where.
		list(local).focus();
		press(list(local), 'ArrowDown', 40);
		press(list(local), 'ArrowDown', 40, true);
		await until(() => !copyButton.disabled, 'Selecting did not offer copy');
		assert(copyButton.getAttribute('aria-label') === 'Copy 2 to Downloads on Studio PC', `Wrong copy label: ${copyButton.getAttribute('aria-label') ?? ''}`);
		assert(local.classList.contains('active') && !remote.classList.contains('active'), 'The pane being worked in is not marked');
		copyButton.click();
		await until(() => session.copies.length === 1, 'Copy did not start');
		assert(session.copies[0].from === 'local' && session.copies[0].items.map(item => item.name).join(',') === 'invoice.pdf,notes.md' && session.copies[0].to.root === '2' && session.copies[0].to.path === 'Users/someone/Downloads', `Wrong copy: ${JSON.stringify(session.copies[0])}`);
		// What is copied shows where it lands, at its place in that folder, before it has arrived.
		await until(() => remote.querySelectorAll('.files-row.arriving').length === 2, 'The copies did not show in the folder they land in');
		assert(names(remote).join(',') === 'Installers,invoice.pdf,notes.md,report-q3.xlsx', `Arriving rows are out of place: ${names(remote).join(',')}`);
		const arriving = remote.querySelector('.files-row.arriving');
		// The selection is one transfer: each of its rows shows how far the transfer got.
		assert(arriving?.querySelector('.files-ring') !== null && arriving?.querySelector('.files-cell-modified')?.textContent === '0 of 2 files', `An arriving row does not show its progress: ${arriving?.textContent ?? ''}`);
		assert(view.element.querySelector('.files-status-transfers')?.textContent.startsWith('Copying to Studio PC') === true, 'The status bar does not sum up the copy');
		// A change the watch sees and a file landing, together, list the folder once; what follows waits a second.
		const listed = remoteDisk.lists.length;
		const watched = performance.now();
		remoteDisk.watchers.get('2:Users/someone/Downloads')?.();
		session.set(session.jobs.map(item => ({ ...item, files_done: '1' })));
		await until(() => remoteDisk.lists.length > listed && remote.querySelector('.files-row.arriving .files-cell-modified')?.textContent === '1 of 2 files', 'The watch and a landing were not both seen');
		// The listing that follows comes a second after the first: only a computer that took that long to show the landing may have it already.
		const lists = remoteDisk.lists.length - listed;
		assert(lists === 1 || (lists === 2 && performance.now() - watched >= 1000), `The watch and a landing listed the folder ${String(lists)} times`);
		// What lands shows on the receiving side without reloading it, tinted for a moment.
		for (const item of session.copies[0].items) {
			remoteDisk.land(session.copies[0].to, entry(item.name, 'file', 1000));
		}
		session.set(session.jobs.map(item => ({ ...item, state: 'Complete', done_bytes: item.total_bytes, files_done: item.files_total })));
		await until(() => remoteDisk.lists.length > listed, 'The receiving pane did not list again when the copy completed');
		await until(() => remote.querySelectorAll('.files-row.arriving').length === 0 && remote.querySelectorAll('.files-row[data-tone="landed"]').length === 2, 'What arrived did not become ordinary rows, tinted for a moment');

		// A name already there asks under that name: answered in place, or with Enter.
		const downloads = { root: '0', path: 'Users/someone/Downloads' };
		session.set([job('21', 'WaitingDecision', { direction: 'Download', label: 'notes.md', waiting_file: 'notes.md', files_total: '1', destination: { ...downloads, items: [{ name: 'notes.md', folder: false }] } })]);
		await until(() => local.querySelector('.files-decision') !== null, 'The clash did not ask under the name');
		assert(local.querySelector('.files-decision-text')?.textContent === 'Studio PC is sending another “notes.md”', `Wrong question: ${local.querySelector('.files-decision-text')?.textContent ?? ''}`);
		assert(local.querySelector('.files-decision-same')?.hasAttribute('hidden') === true, 'One file offers the same answer for others');
		const namesake = [...local.querySelectorAll('.files-row')].find(row => row.querySelector('.files-row-name')?.textContent === 'notes.md');
		assert(namesake?.querySelector('.files-ring') === null, 'The file already there looks like it is being replaced');
		[...local.querySelectorAll<HTMLButtonElement>('.files-decision button')].find(button => button.textContent === 'Replace')?.click();
		await until(() => session.resolutions.length === 1, 'Replace did not answer');
		assert(JSON.stringify(session.resolutions[0]) === JSON.stringify(['21', 'replace', false]), `Wrong answer: ${JSON.stringify(session.resolutions[0])}`);
		const decisionButton = view.element.querySelector<HTMLButtonElement>('.files-status-decision');
		assert(decisionButton?.textContent === '1 needs your decision', 'The status bar does not ask for the decision');
		decisionButton.click();
		await until(() => document.activeElement === list(local) && local.querySelector('.monaco-list-row.focused .files-decision') !== null, 'The status bar did not go to the question');
		answers.push({ button: 2, checked: false });
		press(list(local), 'Enter', 13);
		await until(() => session.resolutions.length === 2, 'Enter on the question did not ask');
		assert(session.resolutions[1][1] === 'keepBoth', `Wrong answer from the dialog: ${JSON.stringify(session.resolutions[1])}`);

		// The status bar opens every transfer, the finished ones too; Escape closes it where it opened.
		session.set([job('20', 'Interrupted', { files_total: '1', destination: { root: '2', path: 'Users/someone/Downloads', items: [{ name: 'big.iso', folder: false }] } }), job('22', 'Complete', { label: 'Screenshots', done_bytes: '1000000' })]);
		const summary = view.element.querySelector<HTMLButtonElement>('.files-status-transfers');
		assert(summary?.textContent === '1 stopped', `A stopped transfer is not said: ${summary?.textContent ?? ''}`);
		summary.click();
		const activity = view.element.querySelector<HTMLElement>('.files-activity');
		assert(activity !== null && !activity.hidden && document.activeElement === activity, 'The list of transfers did not open');
		const headings = [...activity.querySelectorAll('.files-activity-heading')].map(node => node.textContent);
		assert(headings.join(',') === 'Now,Earlier', `Wrong groups: ${headings.join(',')}`);
		const actions = [...activity.querySelectorAll('.files-activity-item button')].map(node => node.textContent).join(',');
		assert(actions === 'Resume', `Wrong actions: ${actions}`);
		press(activity, 'Escape', 27);
		assert(view.element.querySelector<HTMLElement>('.files-activity')?.hidden === true && document.activeElement === summary, 'Escape did not close the list and return to the status bar');
		session.set([]);

		// Rename in place: a taken name is refused and said, the item kept.
		list(local).focus();
		press(list(local), 'Home', 36);
		press(list(local), 'ArrowDown', 40);
		press(list(local), 'F2', 113);
		await until(() => local.querySelector('.files-row-name input') !== null, 'Rename did not open in the row');
		const input = local.querySelector<HTMLInputElement>('.files-row-name input');
		assert(input !== null && input.value === 'invoice.pdf' && input.selectionEnd === 'invoice'.length, 'Rename did not select the name without its extension');
		input.value = 'notes.md';
		input.dispatchEvent(new Event('input'));
		input.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
		await until(() => !(local.querySelector<HTMLElement>('.files-notice')?.hidden ?? true), 'A taken name was not reported');
		assert(local.querySelector('.files-notice')?.textContent === 'That name is already taken here.' && names(local).includes('invoice.pdf'), 'A refused rename lost the item or its reason');

		// Deleting for good asks first, says it is permanent, and Cancel keeps everything.
		list(local).focus();
		press(list(local), 'Delete', 46, true);
		await until(() => dialogs.length === 2, 'Deleting for good did not ask');
		assert(dialogs[1].message.startsWith('Permanently delete “invoice.pdf” from'), `Wrong delete question: ${dialogs[1].message}`);
		assert(localDisk.deletes.length === 0, 'A permanent delete ran without confirmation');
		// The cancelled question settles before it is asked for again.
		await new Promise(resolve => setTimeout(resolve, 0));
		// Asking twice while the question is open asks once.
		const asked = dialogs.length;
		let answer: (result: IDialogResult) => void = () => undefined;
		answers.push(new Promise<IDialogResult>(resolve => {
			answer = resolve;
		}));
		press(list(local), 'Delete', 46, true);
		press(list(local), 'Delete', 46, true);
		await until(() => dialogs.length === asked + 1, 'Deleting for good did not ask again');
		await new Promise(resolve => setTimeout(resolve, 50));
		assert(dialogs.length === asked + 1, `Delete asked ${String(dialogs.length - asked)} times at once`);
		answer({ button: 0, checked: false });
		await until(() => localDisk.deletes.join() === 'invoice.pdf:gone', 'The confirmed delete did not run');
		await until(() => !names(local).includes('invoice.pdf'), 'The deleted item stayed listed');

		// Delete moves to the Trash at once, as Finder and Explorer do; the menu offers both.
		press(list(local), 'Home', 36);
		press(list(local), 'ArrowDown', 40);
		press(list(local), 'Delete', 46);
		await until(() => localDisk.deletes.at(-1) === 'notes.md:trash', 'Delete did not move the item to the Trash');
		assert(dialogs.length === asked + 1, 'Moving to the Trash asked first');
		await until(() => !names(local).includes('notes.md'), 'The item moved to the Trash stayed listed');
		// Where there is no Trash, it offers to delete for good, in the other computer's words.
		remoteDisk.noTrash.add('report-q3.xlsx');
		const sheet = names(remote).indexOf('report-q3.xlsx');
		list(remote).focus();
		press(list(remote), 'Home', 36);
		for (let step = 0; step < sheet; step++) {
			press(list(remote), 'ArrowDown', 40);
		}
		answers.push({ button: 0, checked: false });
		press(list(remote), 'Delete', 46);
		await until(() => remoteDisk.deletes.at(-1) === 'report-q3.xlsx:gone', 'A place with no Trash did not offer to delete for good');
		assert(dialogs.at(-1)?.message === '“report-q3.xlsx” can’t be moved to the Recycle Bin. Delete it permanently?', `Wrong question: ${dialogs.at(-1)?.message ?? ''}`);
		remote.querySelector<HTMLElement>('.monaco-list-row')?.dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, button: 2 }));
		await until(() => shown.menu.some(action => action.id === 'files.delete'), 'The menu did not open on a row');
		assert(shown.menu.find(action => action.id === 'files.delete')?.label === 'Delete' && shown.menu.some(action => action.id === 'files.deletePermanently'), 'The menu does not offer the Recycle Bin and deleting for good');

		// Hidden files, from the pane's menu, are remembered.
		local.querySelector<HTMLButtonElement>('.files-icon-button[aria-label="More Actions"]')?.click();
		run('files.hidden');
		assert(state.get('files.showHidden') === true, 'Show Hidden Files was not remembered');

		// What changes outside VniLink, as a file deleted in Finder or Explorer, shows without a refresh.
		localDisk.removeOutside({ root: '0', path: 'Users/someone/Downloads' }, 'vnilink-demo.mov');
		await until(() => !names(local).includes('vnilink-demo.mov'), 'A file removed outside VniLink stayed listed here');
		const remoteFolder = crumbs(remote).join('/') === 'C:/Users/someone/Downloads' ? { root: '2', path: 'Users/someone/Downloads' } : undefined;
		assert(remoteFolder !== undefined && remoteDisk.watchers.has('2:Users/someone/Downloads'), 'The other computer\'s open folder is not watched');
		remoteDisk.removeOutside(remoteFolder, 'invoice.pdf');
		await until(() => !names(remote).includes('invoice.pdf'), 'A file removed on the other computer stayed listed');

		// A long folder scrolls to its last row, which shows whole above the status bar.
		list(remote).focus();
		press(list(remote), 'Home', 36);
		press(list(remote), 'Enter', 13);
		await until(() => crumbs(remote).at(-1) === 'Installers', 'Installers did not open');
		assert(remoteDisk.watchers.has('2:Users/someone/Downloads/Installers') && !remoteDisk.watchers.has('2:Users/someone/Downloads'), 'The watch did not follow the open folder');
		// A path longer than its bar scrolls, keeping whole names and the open folder in view. The
		// bar itself is narrowed to the open folder's name and its chevron, measured here: names
		// and the buttons beside the bar are wider in some systems' fonts, so a fixed width, or one
		// worked out from the pane's, can leave no room for one whole name.
		const crumbArea = remote.querySelector<HTMLElement>('.files-crumb-area');
		const folderWidth = [...remote.querySelectorAll<HTMLElement>('.files-crumb')].at(-1)?.getBoundingClientRect().width;
		const pathWidth = remote.querySelector<HTMLElement>('.files-crumbs')?.scrollWidth;
		assert(crumbArea !== null && folderWidth !== undefined && pathWidth !== undefined && folderWidth + 24 < pathWidth, 'The open path is too short to scroll');
		crumbArea.style.flex = `0 0 ${String(folderWidth + 24)}px`;
		await new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)));
		const area = remote.querySelector<HTMLElement>('.files-crumb-area')?.getBoundingClientRect();
		const steps = [...remote.querySelectorAll<HTMLElement>('.files-crumb')];
		const end = steps.at(-1)?.getBoundingClientRect();
		assert(steps.every(step => step.scrollWidth <= step.clientWidth), 'A long path squeezed its folder names');
		assert(area !== undefined && end !== undefined && end.right <= area.right + 1 && end.left >= area.left, `The open folder is not in view at the end of a long path: it spans ${String(end?.left)}–${String(end?.right)} in ${String(area?.left)}–${String(area?.right)}`);
		assert((steps[0]?.getBoundingClientRect().left ?? 0) < area.left, 'A long path did not scroll');
		crumbArea.style.flex = '';
		press(list(remote), 'End', 35);
		await until(() => names(remote).includes('setup-59.exe'), 'The end of a long folder is not reachable');
		await new Promise(resolve => requestAnimationFrame(resolve));
		const last = [...remote.querySelectorAll<HTMLElement>('.monaco-list-row')].find(row => row.querySelector('.files-row-name')?.textContent === 'setup-59.exe');
		const box = remote.querySelector<HTMLElement>('.files-list')?.getBoundingClientRect();
		assert(last !== undefined && box !== undefined && last.getBoundingClientRect().bottom <= box.bottom, `The last row is cut off: row ends at ${String(last?.getBoundingClientRect().bottom)}, the list at ${String(box?.bottom)}`);

		// The other computer turned Files off, then the connection ends.
		session.end();
		await until(() => (remote.querySelector('.files-state-title')?.textContent ?? '') === 'Not connected to Studio PC.', 'A lost connection did not show in the pane');
		// Once the computer can be reached again, the pane comes back by itself.
		session.ended = false;
		await until(() => remote.querySelector('.files-state:not([hidden])') === null && names(remote).length > 0, 'The pane did not reconnect by itself');
		session.end();
		await until(() => (remote.querySelector('.files-state-title')?.textContent ?? '') === 'Not connected to Studio PC.', 'The second loss did not show');
		openings.push('turnedOff');
		remote.querySelector<HTMLButtonElement>('.files-state-action')?.click();
		await until(() => (remote.querySelector('.files-state-title')?.textContent ?? '') === 'Studio PC doesn’t share its files.', 'Files turned off there is not said');
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
	const into = { root: '2', path: 'Users/someone/Downloads' };
	session.set([
		job('20', 'Transferring', { label: 'vnilink-demo.mov', files_total: '1', done_bytes: '412000', total_bytes: '1500000', destination: { ...into, items: [{ name: 'vnilink-demo.mov', folder: false }] } }),
		job('21', 'WaitingDecision', { direction: 'Download', waiting_file: 'notes.md', label: 'notes.md', files_total: '1', destination: { root: '0', path: 'Users/someone/Downloads', items: [{ name: 'notes.md', folder: false }] } }),
		job('22', 'Complete', { label: 'Screenshots', done_bytes: '1000000', destination: { ...into, items: [{ name: 'Screenshots', folder: true }] } }),
	]);
	view.element.querySelector<HTMLButtonElement>('.files-status-transfers')?.click();
	return {
		dispose: () => {
			host.remove();
			disposables.dispose();
		},
	};
}
