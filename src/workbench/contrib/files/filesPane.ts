import * as nls from '../../../nls.js';
import { $, addDisposableListener, append, EventType, h } from '../../../base/browser/dom.js';
import { StandardKeyboardEvent } from '../../../base/browser/keyboardEvent.js';
import { DataTransfers, type IDragAndDropData } from '../../../base/browser/dnd.js';
import { renderIcon } from '../../../base/browser/ui/icon/icon.js';
import { InputBox } from '../../../base/browser/ui/inputbox/inputBox.js';
import { DomScrollableElement } from '../../../base/browser/ui/scrollbar/scrollableElement.js';
import { ListDragOverEffectType, type IListDragAndDrop, type IListDragOverReaction, type IListRenderer, type IListVirtualDelegate } from '../../../base/browser/ui/list/list.js';
import { List, type IStyleController } from '../../../base/browser/ui/list/listWidget.js';
import { NativeDragAndDropData } from '../../../base/browser/ui/list/listView.js';
import { webUtils } from '../../../base/parts/sandbox/electron-browser/globals.js';
import { Separator, toAction, type IAction } from '../../../base/common/actions.js';
import { Emitter } from '../../../base/common/event.js';
import { Icons, type IconId } from '../../../base/common/icons.js';
import { KeyCode, KeyMod } from '../../../base/common/keyCodes.js';
import { ScrollbarVisibility } from '../../../base/common/scrollable.js';
import { Disposable, DisposableStore, MutableDisposable, toDisposable } from '../../../base/common/lifecycle.js';
import { isMacintosh } from '../../../base/common/platform.js';
import { IContextMenuService } from '../../../platform/contextview/browser/contextView.js';
import { engineErrorCategory } from '../../../platform/engine/common/engine.js';
import type { FileConflictChoice, FileEntry, FileFolderKind, FilePlaces } from '../../../platform/engine/common/files.js';
import { filesLastFolderKey, type FilesPane, type FolderLocation } from '../../../platform/state/common/uiState.js';
import { defaultInputBoxStyles, defaultListStyles } from '../../../platform/theme/browser/defaultStyles.js';
import { IDialogService } from '../../services/dialogs/dialogService.js';
import type { FilesItem, IFilesDisk } from '../../services/files/filesService.js';
import { IUIStateService } from '../../services/uiState/uiStateService.js';
import { renderRing, type FilesArrival, type TransferRequest } from './filesActivity.js';
import { entrySize, folderName, formatModified, formatSize, itemCount } from './filesText.js';

/** Rows draw their own focus and selection from files.css; the list generates no style rules the CSP would refuse. */
const cssStyleController: IStyleController = {
	style(): void {
		// Selection follows the pane's activity, in files.css.
	},
};

const ROW_HEIGHT = 26;

const DECISION_HEIGHT = 40;

/**
 * One row of a pane: an entry of the folder; a transfer arriving under a
 * name the folder doesn't have yet; or, under either, the question a name
 * already there raises. Only entries are selected, renamed or dragged.
 */
interface PaneRow {
	readonly entry: FileEntry | undefined;
	readonly arrival: FilesArrival | undefined;
	readonly decision: boolean;
}

/** Names never hold NUL, so these ids can't meet an entry's. */
function rowId(row: PaneRow): string {
	return row.entry?.name ?? `\0${row.decision ? 'decision' : 'arriving'}:${row.arrival?.jobId ?? ''}`;
}

function rowName(row: PaneRow): string {
	return row.entry?.name ?? row.arrival?.name ?? '';
}

type SortColumn = 'name' | 'modified' | 'size';

const collator = new Intl.Collator(undefined, { numeric: true, sensitivity: 'base' });

/** Folders first, then by the chosen column, names in natural order. */
export function sortEntries(entries: readonly FileEntry[], column: SortColumn, descending: boolean): FileEntry[] {
	const folder = (entry: FileEntry) => entry.kind === 'dir' ? 0 : 1;
	const compare = (a: FileEntry, b: FileEntry): number => {
		switch (column) {
			case 'name': return collator.compare(a.name, b.name);
			case 'modified': return Number(a.mtimeMs) - Number(b.mtimeMs) || collator.compare(a.name, b.name);
			case 'size': return Number(a.size) - Number(b.size) || collator.compare(a.name, b.name);
		}
	};
	return [...entries].sort((a, b) => folder(a) - folder(b) || (descending ? -compare(a, b) : compare(a, b)));
}

/** A name not yet in `taken`: "New Folder", then "New Folder 2". */
export function freshName(base: string, taken: readonly string[]): string {
	const names = new Set(taken.map(name => name.toLocaleLowerCase()));
	if (!names.has(base.toLocaleLowerCase())) {
		return base;
	}
	let index = 2;
	while (names.has(`${base} ${String(index)}`.toLocaleLowerCase())) {
		index++;
	}
	return `${base} ${String(index)}`;
}

function folderLabel(kind: FileFolderKind, path: string): string {
	switch (kind) {
		case 'home': return nls.localize('files.home', 'Home');
		case 'desktop': return nls.localize('files.desktop', 'Desktop');
		case 'documents': return nls.localize('files.documents', 'Documents');
		case 'downloads': return nls.localize('files.downloads', 'Downloads');
		case 'pictures': return nls.localize('files.pictures', 'Pictures');
		case 'volume': return path.slice(path.lastIndexOf('/') + 1);
	}
}

function same(a: FolderLocation | undefined, b: FolderLocation | undefined): boolean {
	if (!a || !b) {
		return false;
	}
	return a.root === b.root && a.path === b.path;
}

function child(folder: FolderLocation, name: string): FolderLocation {
	return { root: folder.root, path: folder.path ? `${folder.path}/${name}` : name };
}

function parent(folder: FolderLocation): FolderLocation | undefined {
	return folder.path ? { root: folder.root, path: folder.path.includes('/') ? folder.path.slice(0, folder.path.lastIndexOf('/')) : '' } : undefined;
}

/** Whether a drag carries files from the system's file manager. */
function carriesFiles(event: DragEvent): boolean {
	return [...event.dataTransfer?.types ?? []].includes(DataTransfers.FILES);
}

/** What is being dragged out of a pane, while it is. */
let dragging: { readonly pane: FilesPaneView; readonly items: readonly FilesItem[] } | undefined;

interface RowTemplate {
	readonly row: HTMLElement;
	readonly icon: HTMLElement;
	readonly name: HTMLElement;
	readonly modified: HTMLElement;
	readonly size: HTMLElement;
	readonly editing: MutableDisposable<InputBox>;
}

class RowDelegate implements IListVirtualDelegate<PaneRow> {
	public getHeight(row: PaneRow): number {
		return row.decision ? DECISION_HEIGHT : ROW_HEIGHT;
	}

	public getTemplateId(row: PaneRow): string {
		return row.decision ? 'decision' : 'file';
	}
}

class RowRenderer implements IListRenderer<PaneRow, RowTemplate> {
	public readonly templateId = 'file';

	constructor(private readonly pane: FilesPaneView) { }

	public renderTemplate(container: HTMLElement): RowTemplate {
		const row = h('.files-row', [
			h('span.files-cell.files-cell-name', [h('span.files-row-icon@icon'), h('span.files-row-name@name')]),
			h('span.files-cell.files-cell-modified@modified'),
			h('span.files-cell.files-cell-size@size'),
		]);
		append(container, row.root);
		return { row: row.root, icon: row.icon, name: row.name, modified: row.modified, size: row.size, editing: new MutableDisposable() };
	}

	public renderElement(row: PaneRow, _index: number, template: RowTemplate): void {
		const { entry, arrival } = row;
		const ring = arrival && arrival.tone !== 'landed';
		template.row.classList.toggle('arriving', !entry);
		if (arrival) {
			template.row.dataset.tone = arrival.tone;
		} else {
			delete template.row.dataset.tone;
		}
		template.row.dataset.kind = entry?.kind ?? (arrival?.folder ? 'dir' : 'file');
		const folder = template.row.dataset.kind === 'dir';
		template.icon.replaceChildren(ring ? renderRing(arrival.progress, arrival.tone) : renderIcon(folder ? Icons.files : Icons.file, { variant: folder ? 'filled' : 'outlined' }));
		template.modified.textContent = ring || !entry ? arrival?.status ?? '' : formatModified(Number(entry.mtimeMs), new Date());
		template.modified.title = ring ? arrival.status : '';
		template.size.textContent = entry ? entrySize(entry) : folder ? '—' : formatSize(arrival?.bytes ?? 0);
		if (!entry) {
			template.editing.clear();
			template.name.textContent = arrival?.name ?? '';
			template.name.title = arrival?.name ?? '';
		} else if (this.pane.editing === entry.name) {
			template.name.replaceChildren();
			template.editing.value = this.pane.renameBox(template.name, entry);
		} else {
			template.editing.clear();
			template.name.textContent = entry.name;
			template.name.title = entry.name;
		}
	}

	public disposeElement(_row: PaneRow, _index: number, template: RowTemplate): void {
		template.editing.clear();
	}

	public disposeTemplate(template: RowTemplate): void {
		template.editing.dispose();
	}
}

interface DecisionTemplate {
	readonly text: HTMLElement;
	readonly same: HTMLInputElement;
	readonly sameLabel: HTMLElement;
	readonly buttons: readonly (readonly [HTMLButtonElement, FileConflictChoice])[];
	readonly listeners: DisposableStore;
}

/** The question under a name already there, answered in place. */
class DecisionRenderer implements IListRenderer<PaneRow, DecisionTemplate> {
	public readonly templateId = 'decision';

	constructor(private readonly pane: FilesPaneView) { }

	public renderTemplate(container: HTMLElement): DecisionTemplate {
		const root = append(container, $('.files-decision', { role: 'group' }));
		const text = append(root, $('span.files-decision-text'));
		const sameLabel = append(root, $('label.files-decision-same'));
		const same = append(sameLabel, $<HTMLInputElement>('input', { type: 'checkbox' }));
		append(sameLabel, $('span', undefined, nls.localize('files.sameForAll', 'Same for the others')));
		const buttons = ([['skip', nls.localize('files.skip', 'Skip')], ['keepBoth', nls.localize('files.keepBoth', 'Keep Both')], ['replace', nls.localize('files.replace', 'Replace')]] as const).map(([choice, label]) => {
			const button = append(root, $<HTMLButtonElement>('button.files-text-button', { type: 'button' }));
			button.textContent = label;
			button.classList.toggle('primary', choice === 'replace');
			return [button, choice] as const;
		});
		return { text, same, sameLabel, buttons, listeners: new DisposableStore() };
	}

	public renderElement(row: PaneRow, _index: number, template: DecisionTemplate): void {
		template.listeners.clear();
		const arrival = row.arrival;
		const decision = arrival?.decision;
		if (!arrival || !decision) {
			return;
		}
		template.text.textContent = decision.text;
		template.text.title = decision.text;
		template.text.parentElement?.setAttribute('aria-label', decision.text);
		template.sameLabel.hidden = !decision.many;
		template.same.checked = false;
		for (const [button, choice] of template.buttons) {
			// The list must not take the press as choosing the row.
			template.listeners.add(addDisposableListener(button, EventType.MOUSE_DOWN, event => {
				event.stopPropagation();
			}));
			template.listeners.add(addDisposableListener(button, EventType.CLICK, () => {
				this.pane.requestTransfer({ jobId: arrival.jobId, choice, applyToRemaining: decision.many && template.same.checked });
			}));
		}
		template.listeners.add(addDisposableListener(template.sameLabel, EventType.MOUSE_DOWN, event => {
			event.stopPropagation();
		}));
	}

	public disposeElement(_row: PaneRow, _index: number, template: DecisionTemplate): void {
		template.listeners.clear();
	}

	public disposeTemplate(template: DecisionTemplate): void {
		template.listeners.dispose();
	}
}

class PaneDragAndDrop implements IListDragAndDrop<PaneRow> {
	constructor(private readonly pane: FilesPaneView) { }

	public getDragURI(row: PaneRow): string | null {
		return row.entry?.name ?? null;
	}

	public getDragLabel(rows: PaneRow[]): string | undefined {
		const entries = rows.flatMap(row => row.entry ? [row.entry] : []);
		return entries.length === 1 ? entries[0].name : itemCount(entries.length);
	}

	public onDragStart(data: IDragAndDropData): void {
		const entries = (data.getData() as PaneRow[]).flatMap(row => row.entry ? [row.entry] : []);
		const folder = this.pane.folder;
		dragging = folder && entries.length ? { pane: this.pane, items: entries.map(entry => ({ folder, name: entry.name })) } : undefined;
	}

	public onDragOver(
		data: IDragAndDropData,
		row: PaneRow | undefined,
		targetIndex: number | undefined,
		_sector: unknown,
		event: DragEvent,
	): boolean | IListDragOverReaction {
		// From Finder or Explorer: this computer's files copy to the other one.
		const external = data instanceof NativeDragAndDropData && carriesFiles(event);
		if (external && this.pane.side === 'local') {
			this.pane.showDropHint();
			return false;
		}
		const count = external ? event.dataTransfer?.items.length ?? 0 : dragging?.pane !== this.pane ? dragging?.items.length ?? 0 : 0;
		if (!count || !this.pane.folder) {
			return false;
		}
		const target = row?.entry;
		const into = target?.kind === 'dir' ? target : undefined;
		this.pane.showDrop(count, into?.name);
		return { accept: true, effect: { type: ListDragOverEffectType.Copy }, feedback: into && targetIndex !== undefined ? [targetIndex] : [-1] };
	}

	public onDragLeave(): void {
		this.pane.showDrop(0, undefined);
	}

	public drop(data: IDragAndDropData, row: PaneRow | undefined, _index: number | undefined, _sector: unknown, event: DragEvent): void {
		const source = dragging;
		this.pane.showDrop(0, undefined);
		const folder = this.pane.folder;
		const target = row?.entry;
		const into = folder && (target?.kind === 'dir' ? child(folder, target.name) : folder);
		if (!into) {
			return;
		}
		if (data instanceof NativeDragAndDropData && this.pane.side === 'remote') {
			const paths = [...event.dataTransfer?.files ?? []].map(file => webUtils.getPathForFile(file)).filter(path => path !== '');
			if (paths.length) {
				this.pane.droppedFromSystem(paths, into);
			}
		} else if (source && source.pane !== this.pane) {
			this.pane.dropped(source.items, into);
		}
	}

	public onDragEnd(): void {
		dragging = undefined;
		this.pane.showDrop(0, undefined);
	}

	public dispose(): void {
		// Holds no resources of its own.
	}
}

/**
 * One computer's disk in a Files tab: where it is, what is there, and the
 * file manager's own operations. Selection shows in the accent colour while
 * the pane is the one being worked in.
 */
export class FilesPaneView extends Disposable {
	public readonly element: HTMLElement;
	private readonly changedSelection = this._register(new Emitter<void>());
	public readonly onDidChangeSelection = this.changedSelection.event;
	private readonly activated = this._register(new Emitter<void>());
	/** Fires when the pane is clicked or focused. */
	public readonly onDidActivate = this.activated.event;
	private readonly requestedCopy = this._register(new Emitter<void>());
	/** Fires for the copy shortcut. */
	public readonly onDidRequestCopy = this.requestedCopy.event;
	private readonly droppedItems = this._register(new Emitter<{ readonly items: readonly FilesItem[]; readonly into: FolderLocation }>());
	/** Fires when items dragged from the other pane are dropped here. */
	public readonly onDidDrop = this.droppedItems.event;
	private readonly droppedPaths = this._register(new Emitter<{ readonly paths: readonly string[]; readonly into: FolderLocation }>());
	/** Fires when files from Finder or Explorer are dropped here, by their paths on this computer. */
	public readonly onDidDropFromSystem = this.droppedPaths.event;
	private readonly navigated = this._register(new Emitter<void>());
	public readonly onDidNavigate = this.navigated.event;
	private readonly transferRequested = this._register(new Emitter<TransferRequest>());
	/** Fires for something done to a transfer from its row. */
	public readonly onDidRequestTransfer = this.transferRequested.event;
	private readonly nameButton: HTMLButtonElement;
	private readonly crumbs: HTMLElement;
	private readonly crumbScroll: DomScrollableElement;
	private readonly crumbListeners = this._register(new DisposableStore());
	private readonly backButton: HTMLButtonElement;
	private readonly forwardButton: HTMLButtonElement;
	private readonly upButton: HTMLButtonElement;
	private readonly filterSlot: HTMLElement;
	private readonly filterBox: InputBox;
	private readonly filterButton: HTMLButtonElement;
	private readonly sortButtons = new Map<SortColumn, HTMLButtonElement>();
	private readonly listContainer: HTMLElement;
	private readonly list: List<PaneRow>;
	private readonly state: HTMLElement;
	private readonly stateTitle: HTMLElement;
	private readonly stateBody: HTMLElement;
	private readonly stateAction: HTMLButtonElement;
	private readonly dropBar: HTMLElement;
	private readonly notice: HTMLElement;
	private disk: IFilesDisk | undefined;
	private places: FilePlaces | undefined;
	private current: FolderLocation | undefined;
	private entries: readonly FileEntry[] = [];
	/** The entries shown, sorted, without the transfers among them. */
	private shown: FileEntry[] = [];
	private rows: PaneRow[] = [];
	private arrivals: readonly FilesArrival[] = [];
	private readonly back: FolderLocation[] = [];
	private readonly forward: FolderLocation[] = [];
	private revision = 0;
	private sort: SortColumn = 'name';
	private descending = false;
	private filter = '';
	private showHidden = false;
	private truncated = false;
	public editing: string | undefined;
	/** Where a Shift selection starts. */
	private anchor: number | undefined;
	private confirming = false;
	private stateAct: (() => void) | undefined;

	constructor(
		public readonly side: FilesPane,
		private readonly deviceId: string,
		/** The computer this pane shows: "This Mac", or the paired computer's name. */
		public readonly computer: string,
		private readonly os: 'macos' | 'windows' | 'linux' | 'other',
		@IContextMenuService private readonly contextMenuService: IContextMenuService,
		@IDialogService private readonly dialogService: IDialogService,
		@IUIStateService private readonly uiState: IUIStateService,
	) {
		super();
		this.element = $('section.files-pane');
		this.element.setAttribute('aria-label', computer);
		const bar = append(this.element, $('.files-path-bar'));
		this.backButton = this.iconButton(bar, Icons.back, nls.localize('files.back', 'Back'));
		this.forwardButton = this.iconButton(bar, Icons.forward, nls.localize('files.forward', 'Forward'));
		this.upButton = this.iconButton(bar, Icons.up, nls.localize('files.up', 'Enclosing Folder'));
		this.nameButton = append(bar, $<HTMLButtonElement>('button.files-computer', { type: 'button', 'aria-haspopup': 'menu' }));
		const chevron = renderIcon(Icons.chevronDown);
		chevron.classList.add('files-computer-chevron');
		append(this.nameButton, renderIcon(os === 'windows' ? Icons.windows : os === 'linux' ? Icons.linux : os === 'macos' ? Icons.apple : Icons.computers), $('span', undefined, computer), chevron);
		this.nameButton.title = nls.localize('files.places', 'Go to a place on {0}', computer);
		this.crumbs = $('nav.files-crumbs', { 'aria-label': nls.localize('files.path', 'Location') });
		// As VS Code's breadcrumbs: a long path scrolls sideways, with the wheel too, and never squeezes its names.
		this.crumbScroll = this._register(new DomScrollableElement(this.crumbs, {
			vertical: ScrollbarVisibility.Hidden,
			horizontal: ScrollbarVisibility.Auto,
			horizontalScrollbarSize: 3,
			useShadows: false,
			scrollYToX: true,
		}));
		const crumbArea = append(bar, this.crumbScroll.getDomNode());
		crumbArea.classList.add('files-crumb-area');
		const tools = append(bar, $('.files-tools'));
		this.filterButton = this.iconButton(tools, Icons.search, nls.localize('files.filter', 'Filter'));
		this.filterSlot = append(tools, $('.files-filter'));
		this.filterBox = this._register(new InputBox(this.filterSlot, undefined, { inputBoxStyles: defaultInputBoxStyles, placeholder: nls.localize('files.filterPlaceholder', 'Filter'), ariaLabel: nls.localize('files.filterLabel', 'Filter this folder') }));
		this.filterSlot.hidden = true;
		const refresh = this.iconButton(tools, Icons.refresh, nls.localize('files.refresh', 'Refresh'));
		const more = this.iconButton(tools, Icons.more, nls.localize('files.more', 'More Actions'));
		more.setAttribute('aria-haspopup', 'menu');
		const columns = append(this.element, $('.files-columns', { role: 'row' }));
		for (const [column, label] of [['name', nls.localize('files.name', 'Name')], ['modified', nls.localize('files.modified', 'Modified')], ['size', nls.localize('files.size', 'Size')]] as const) {
			const button = append(columns, $<HTMLButtonElement>(`button.files-sort.files-sort-${column}`, { type: 'button', role: 'columnheader' }));
			append(button, $('span', undefined, label), renderIcon(Icons.chevronUp));
			this.sortButtons.set(column, button);
			this._register(addDisposableListener(button, EventType.CLICK, () => {
				this.descending = this.sort === column ? !this.descending : false;
				this.sort = column;
				this.show();
			}));
		}
		this.listContainer = append(this.element, $('.files-list'));
		this.list = this._register(new List('Files', this.listContainer, new RowDelegate(), [new RowRenderer(this), new DecisionRenderer(this)], {
			...defaultListStyles,
			styleController: () => cssStyleController,
			setRowLineHeight: false,
			paddingTop: 4,
			paddingBottom: 8,
			multipleSelectionSupport: true,
			identityProvider: { getId: rowId },
			keyboardNavigationLabelProvider: { getKeyboardNavigationLabel: row => this.editing === undefined && !row.decision ? rowName(row) : undefined },
			typeNavigationEnabled: true,
			dnd: new PaneDragAndDrop(this),
			accessibilityProvider: {
				getAriaLabel: row => {
					const { entry, arrival } = row;
					if (row.decision && arrival?.decision) {
						return nls.localize('files.decisionRow', '{0}. Press Enter to choose.', arrival.decision.text);
					}
					if (arrival && arrival.tone !== 'landed') {
						return nls.localize('files.arrivingRow', '{0}, arriving, {1}', arrival.name, arrival.status);
					}
					if (!entry) {
						return arrival?.name ?? '';
					}
					return entry.kind === 'dir'
						? nls.localize('files.folderRow', '{0}, folder, modified {1}', entry.name, formatModified(Number(entry.mtimeMs), new Date()))
						: nls.localize('files.fileRow', '{0}, {1}, modified {2}', entry.name, entrySize(entry), formatModified(Number(entry.mtimeMs), new Date()));
				},
				getWidgetAriaLabel: () => nls.localize('files.listLabel', 'Files on {0}', computer),
			},
		}));
		this.state = append(this.element, $('.files-state', { role: 'status' }));
		this.stateTitle = append(this.state, $('p.files-state-title'));
		this.stateBody = append(this.state, $('p.files-state-body'));
		this.stateAction = append(this.state, $<HTMLButtonElement>('button.files-state-action', { type: 'button' }));
		this.state.hidden = true;
		this.dropBar = append(this.element, $('.files-drop-bar', { 'aria-live': 'polite' }));
		this.dropBar.hidden = true;
		this.notice = append(this.element, $('p.files-notice', { role: 'alert' }));
		this.notice.hidden = true;
		this._register(addDisposableListener(this.backButton, EventType.CLICK, () => {
			this.goBack();
		}));
		this._register(addDisposableListener(this.forwardButton, EventType.CLICK, () => {
			this.goForward();
		}));
		this._register(addDisposableListener(this.upButton, EventType.CLICK, () => {
			this.goUp();
		}));
		this._register(addDisposableListener(this.nameButton, EventType.CLICK, () => {
			void this.showPlaces();
		}));
		this._register(addDisposableListener(refresh, EventType.CLICK, () => {
			void this.refresh();
		}));
		this._register(addDisposableListener(more, EventType.CLICK, () => {
			this.showMenu(more, undefined);
		}));
		this._register(addDisposableListener(this.filterButton, EventType.CLICK, () => {
			this.filterSlot.hidden = !this.filterSlot.hidden;
			this.filterButton.setAttribute('aria-expanded', String(!this.filterSlot.hidden));
			if (this.filterSlot.hidden) {
				this.filterBox.value = '';
			} else {
				this.filterBox.focus();
			}
		}));
		this._register(this.filterBox.onDidChange(value => {
			this.filter = value.toLocaleLowerCase();
			this.show();
		}));
		this._register(addDisposableListener(this.stateAction, EventType.CLICK, () => {
			this.stateAct?.();
		}));
		this._register(addDisposableListener(this.element, EventType.FOCUS_IN, () => {
			this.activated.fire();
		}));
		this._register(addDisposableListener(this.element, EventType.MOUSE_DOWN, () => {
			this.activated.fire();
		}));
		this._register(this.list.onDidChangeSelection(event => {
			// A transfer's rows are not things in the folder to select.
			const entries = event.indexes.filter(index => this.rows[index]?.entry !== undefined);
			if (entries.length !== event.indexes.length) {
				this.list.setSelection(entries, event.browserEvent);
				return;
			}
			this.changedSelection.fire();
		}));
		// As in the system's file lists, the selection follows the arrow
		// keys and typed names; Shift extends it, Cmd or Ctrl only moves.
		this._register(this.list.onDidChangeFocus(event => {
			const key = event.browserEvent;
			const focus = event.indexes.at(0);
			if (!(key instanceof KeyboardEvent) || focus === undefined || key.metaKey || key.ctrlKey) {
				return;
			}
			if (key.shiftKey) {
				const anchor = this.anchor ?? focus;
				const from = Math.min(anchor, focus);
				this.list.setSelection(Array.from({ length: Math.abs(focus - anchor) + 1 }, (_, index) => from + index), key);
			} else {
				this.anchor = focus;
				this.list.setSelection([focus], key);
			}
		}));
		this._register(this.list.onMouseDblClick(event => {
			if (event.element) {
				this.open(event.element);
			}
		}));
		this._register(this.list.onContextMenu(event => {
			event.browserEvent.preventDefault();
			const anchor = event.anchor instanceof HTMLElement ? event.anchor : { x: event.anchor.posx, y: event.anchor.posy };
			if (event.element && !event.element.entry) {
				this.showTransferMenu(anchor, event.element);
				return;
			}
			if (event.element && event.index !== undefined && !this.list.getSelection().includes(event.index)) {
				this.list.setSelection([event.index]);
			}
			this.showMenu(anchor, event.element?.entry);
		}));
		this._register(this.list.onKeyDown(event => {
			this.onKey(new StandardKeyboardEvent(event));
		}));
		const resize = new ResizeObserver(() => {
			this.list.layout(this.listContainer.clientHeight, this.listContainer.clientWidth);
			this.revealLastCrumb();
		});
		resize.observe(this.listContainer);
		resize.observe(crumbArea);
		this._register(toDisposable(() => {
			resize.disconnect();
		}));
		this.updateButtons();
	}

	public get folder(): FolderLocation | undefined {
		return this.current;
	}

	/** The pane's places, once its disk has answered. */
	public get knownPlaces(): FilePlaces | undefined {
		return this.places;
	}

	/** Selected entries, as items to copy. */
	public selectedItems(): FilesItem[] {
		const folder = this.current;
		return folder ? this.selectedEntries().map(entry => ({ folder, name: entry.name })) : [];
	}

	/** Entries shown, and the selection's count and file bytes. */
	public summary(): { readonly shown: number; readonly selected: number; readonly bytes: number; readonly truncated: boolean } {
		const selected = this.selectedEntries();
		return { shown: this.shown.length, selected: selected.length, bytes: selected.reduce((sum, entry) => sum + (entry.kind === 'file' ? Number(entry.size) : 0), 0), truncated: this.truncated };
	}

	public setActive(active: boolean): void {
		this.element.classList.toggle('active', active);
	}

	public focus(): void {
		if (this.list.length) {
			if (!this.list.getFocus().length) {
				this.list.setFocus([0]);
			}
			this.list.domFocus();
		} else {
			this.nameButton.focus();
		}
	}

	/** Opens `disk` at the remembered folder if it still opens, else at home. */
	public async start(disk: IFilesDisk): Promise<void> {
		this.disk = disk;
		const revision = ++this.revision;
		this.showState(nls.localize('files.reading', 'Reading {0}…', this.computer), '');
		try {
			this.places = await disk.places();
		} catch (error) {
			if (revision === this.revision) {
				this.showFailure(error);
			}
			return;
		}
		this.showHidden = await this.uiState.get('files.showHidden') ?? false;
		const remembered = await this.uiState.get(filesLastFolderKey(this.deviceId, this.side));
		const home = this.places.folders.find(folder => folder.kind === 'home');
		const fallback = home ? { root: home.root, path: home.path } : this.places.roots.at(0) && { root: this.places.roots[0].id, path: '' };
		if (revision !== this.revision) {
			return;
		}
		if (typeof remembered === 'object' && await this.navigate(remembered, false)) {
			return;
		}
		if (fallback) {
			await this.navigate(fallback, false);
		} else {
			this.showState(nls.localize('files.nothingShared', 'Nothing on {0} is shared.', this.computer), '');
		}
	}

	/** Shows why the disk can't be reached, with an action when one helps. */
	public setUnavailable(title: string, body: string, action?: { readonly label: string; readonly run: () => void }): void {
		this.revision++;
		this.disk = undefined;
		this.entries = [];
		this.show();
		this.showState(title, body, action);
	}

	public async refresh(): Promise<void> {
		if (this.current) {
			await this.navigate(this.current, false);
		}
	}

	/** Lists `folder`; only the newest navigation paints. Resolves whether it opened. */
	public async navigate(folder: FolderLocation, record = true): Promise<boolean> {
		const disk = this.disk;
		if (!disk) {
			return false;
		}
		const revision = ++this.revision;
		const slow = setTimeout(() => {
			if (revision === this.revision) {
				this.showState(nls.localize('files.readingFolder', 'Reading {0}…', folderName(folder, this.places, this.computer)), '');
			}
		}, 150);
		try {
			const listing = await disk.list(folder);
			if (revision !== this.revision) {
				return true;
			}
			if (record && this.current && !same(this.current, folder)) {
				this.back.push(this.current);
				this.forward.length = 0;
			}
			const entering = !same(this.current, folder);
			this.current = folder;
			this.entries = listing.entries;
			this.truncated = listing.truncated;
			this.state.hidden = true;
			this.show(entering);
			this.renderCrumbs();
			this.updateButtons();
			void this.uiState.set(filesLastFolderKey(this.deviceId, this.side), folder);
			this.navigated.fire();
			return true;
		} catch (error) {
			if (revision === this.revision && this.current === undefined) {
				this.showFailure(error);
			} else if (revision === this.revision) {
				this.say(this.failureText(error));
			}
			return false;
		} finally {
			clearTimeout(slow);
		}
	}

	/** Shows or hides the drop bar for `count` items dragged over this pane. */
	public showDrop(count: number, into: string | undefined): void {
		this.dropBar.hidden = count === 0;
		this.element.classList.toggle('drop-target', count > 0);
		if (count && this.current) {
			const where = into ?? folderName(this.current, this.places, this.computer);
			this.dropBar.textContent = count === 1
				? nls.localize('files.dropOne', 'Drop to copy 1 item into {0} on {1}', where, this.computer)
				: nls.localize('files.dropMany', 'Drop to copy {0} items into {1} on {2}', count, where, this.computer);
		}
	}

	public dropped(items: readonly FilesItem[], into: FolderLocation): void {
		this.droppedItems.fire({ items, into });
	}

	public droppedFromSystem(paths: readonly string[], into: FolderLocation): void {
		this.droppedPaths.fire({ paths, into });
	}

	/** This computer's own pane can't take its own files: say where they go instead. */
	public showDropHint(): void {
		this.dropBar.hidden = false;
		this.element.classList.remove('drop-target');
		this.dropBar.textContent = nls.localize('files.dropOtherSide', 'Drop on the other computer’s side to copy these there');
	}

	/** A box in the row being renamed; Enter renames, Escape keeps the name. */
	public renameBox(container: HTMLElement, entry: FileEntry): InputBox {
		const box = new InputBox(container, undefined, { inputBoxStyles: defaultInputBoxStyles, ariaLabel: nls.localize('files.renameLabel', 'New name for {0}', entry.name) });
		box.value = entry.name;
		const dot = entry.kind === 'dir' ? -1 : entry.name.lastIndexOf('.');
		box.inputElement.setSelectionRange(0, dot > 0 ? dot : entry.name.length);
		requestAnimationFrame(() => {
			box.focus();
		});
		let done = false;
		const finish = (commit: boolean): void => {
			if (done) {
				return;
			}
			done = true;
			const name = box.value.trim();
			this.editing = undefined;
			this.show();
			this.list.domFocus();
			if (commit && name && name !== entry.name) {
				void this.rename(entry, name);
			}
		};
		box.inputElement.addEventListener('keydown', event => {
			event.stopPropagation();
			if (event.key === 'Enter') {
				finish(true);
			} else if (event.key === 'Escape') {
				finish(false);
			}
		});
		box.inputElement.addEventListener('blur', () => {
			finish(true);
		});
		return box;
	}

	private iconButton(container: HTMLElement, icon: IconId, label: string): HTMLButtonElement {
		const button = append(container, $<HTMLButtonElement>('button.files-icon-button', { type: 'button', title: label, 'aria-label': label }));
		append(button, renderIcon(icon));
		return button;
	}

	/** Transfers landing here, by the name each lands under; the pane shows them among its entries. */
	public setArrivals(arrivals: readonly FilesArrival[]): void {
		const shape = (list: readonly FilesArrival[]) => list.map(arrival => `${arrival.jobId}:${arrival.name}:${arrival.tone}:${String(arrival.decision !== undefined)}`).join('\n');
		const moved = shape(arrivals) !== shape(this.arrivals);
		this.arrivals = arrivals;
		if (moved) {
			this.show();
			return;
		}
		// Progress alone: only the transfers' own rows are drawn again.
		this.rows.forEach((row, index) => {
			const arrival = row.arrival && arrivals.find(candidate => candidate.jobId === row.arrival?.jobId);
			if (arrival && arrival !== row.arrival) {
				this.rows[index] = { ...row, arrival };
				this.list.splice(index, 1, [this.rows[index]]);
			}
		});
	}

	/** Goes to the row named `name`: selects an entry, or focuses a transfer's question. */
	public reveal(name: string): void {
		const index = this.rows.findIndex(row => rowName(row) === name && !row.decision);
		if (index < 0) {
			return;
		}
		const question = this.rows[index + 1]?.decision ? index + 1 : index;
		this.list.setFocus([question]);
		this.list.setSelection(this.rows[index].entry ? [index] : []);
		this.list.reveal(question);
		this.list.domFocus();
	}

	/** Passes on what was done to a transfer from its row. */
	public requestTransfer(request: TransferRequest): void {
		this.transferRequested.fire(request);
	}

	private selectedEntries(): FileEntry[] {
		return this.list.getSelectedElements().flatMap(row => row.entry ? [row.entry] : []);
	}

	private matches(name: string): boolean {
		return (this.showHidden || !name.startsWith('.')) && (!this.filter || name.toLocaleLowerCase().includes(this.filter));
	}

	/**
	 * Paints the visible entries, hidden files and the filter applied,
	 * sorted, with the transfers landing here among them: one arriving
	 * under a name not here yet takes its sorted place, and a question
	 * about a name already here sits under it.
	 */
	private show(entering = false): void {
		this.shown = sortEntries(this.entries.filter(entry => this.matches(entry.name)), this.sort, this.descending);
		const names = new Set(this.entries.map(entry => entry.name));
		const arriving = new Map<string, FilesArrival>();
		const decorating = new Map<string, FilesArrival>();
		for (const arrival of this.arrivals) {
			if (!this.matches(arrival.name)) {
				continue;
			}
			(names.has(arrival.name) ? decorating : arriving).set(arrival.name, arrival);
		}
		// Not there yet: placed as an entry of its size, modified now, would be.
		const placeholders = [...arriving.values()].map((arrival): FileEntry => ({ name: arrival.name, kind: arrival.folder ? 'dir' : 'file', size: String(arrival.bytes), mtimeMs: String(Date.now()), token: { dev: null, ino: null } }));
		const placed = new Set(placeholders);
		this.rows = sortEntries([...this.shown, ...placeholders], this.sort, this.descending).flatMap((entry): PaneRow[] => {
			const arrival = placed.has(entry) ? arriving.get(entry.name) : decorating.get(entry.name);
			// A single file meeting its namesake leaves that file as it is: only the question is new.
			const decorate = arrival && !(arrival.decision && !arrival.folder && !placed.has(entry));
			const row: PaneRow = { entry: placed.has(entry) ? undefined : entry, arrival: decorate ? arrival : undefined, decision: false };
			return arrival?.decision ? [row, { entry: undefined, arrival, decision: true }] : [row];
		});
		const selected = new Set(entering ? [] : this.selectedEntries().map(entry => entry.name));
		this.list.splice(0, this.list.length, this.rows);
		this.list.setSelection(this.rows.flatMap((row, index) => row.entry && selected.has(row.entry.name) ? [index] : []));
		if (entering) {
			this.list.setFocus(this.rows.length ? [0] : []);
			this.list.scrollTop = 0;
		}
		for (const [column, button] of this.sortButtons) {
			button.setAttribute('aria-sort', column === this.sort ? (this.descending ? 'descending' : 'ascending') : 'none');
			button.classList.toggle('sorted', column === this.sort);
			button.classList.toggle('descending', column === this.sort && this.descending);
		}
		if (this.current && !this.rows.length && this.state.hidden) {
			this.showState(this.filter ? nls.localize('files.noMatch', 'Nothing here matches “{0}”.', this.filterBox.value) : nls.localize('files.empty', 'This folder is empty.'), '');
		} else if (this.rows.length && this.disk) {
			this.state.hidden = true;
		}
		this.listContainer.dataset.truncated = String(this.truncated);
		this.changedSelection.fire();
	}

	private showState(title: string, body: string, action?: { readonly label: string; readonly run: () => void }): void {
		this.state.hidden = false;
		this.stateTitle.textContent = title;
		this.stateBody.textContent = body;
		this.stateBody.hidden = !body;
		this.stateAction.hidden = !action;
		this.stateAction.textContent = action?.label ?? '';
		this.stateAct = action?.run;
	}

	private failureText(error: unknown): string {
		switch (engineErrorCategory(error) ?? '') {
			case 'TransferDenied': return nls.localize('files.notShared', '{0} doesn’t share its files.', this.computer);
			case 'NoSession':
			case 'StaleSession': return nls.localize('files.lost', 'The connection to {0} was lost.', this.computer);
			case 'FileExists': return nls.localize('files.taken', 'That name is already taken here.');
			case 'Timeout': return nls.localize('files.timeout', '{0} didn’t answer in time.', this.computer);
			default: return nls.localize('files.cantOpen', 'This folder can’t be opened.');
		}
	}

	private showFailure(error: unknown): void {
		this.showState(this.failureText(error), '', { label: nls.localize('files.tryAgain', 'Try Again'), run: () => void this.retry() });
	}

	private async retry(): Promise<void> {
		if (this.disk) {
			await (this.current ? this.navigate(this.current, false) : this.start(this.disk));
		}
	}

	/** A short message under the list for a refused operation. */
	public say(message: string): void {
		this.notice.textContent = message;
		this.notice.hidden = false;
		setTimeout(() => {
			if (this.notice.textContent === message) {
				this.notice.hidden = true;
			}
		}, 6000);
	}

	private renderCrumbs(): void {
		const folder = this.current;
		this.crumbListeners.clear();
		this.crumbs.replaceChildren();
		if (!folder) {
			return;
		}
		const root = this.places?.roots.find(candidate => candidate.id === folder.root);
		const parts = folder.path ? folder.path.split('/') : [];
		const steps: { label: string; target: FolderLocation }[] = [];
		if (root && root.name !== '/') {
			steps.push({ label: root.name, target: { root: folder.root, path: '' } });
		}
		parts.forEach((part, index) => {
			steps.push({ label: part, target: { root: folder.root, path: parts.slice(0, index + 1).join('/') } });
		});
		steps.forEach((step, index) => {
			append(this.crumbs, renderIcon(Icons.chevronRight));
			const button = append(this.crumbs, $<HTMLButtonElement>('button.files-crumb', { type: 'button' }));
			button.textContent = step.label;
			if (index === steps.length - 1) {
				button.setAttribute('aria-current', 'location');
			}
			this.crumbListeners.add(addDisposableListener(button, EventType.CLICK, () => {
				void this.navigate(step.target);
			}));
		});
		this.revealLastCrumb();
	}

	/** Keeps the open folder, the path's end, in view. */
	private revealLastCrumb(): void {
		this.crumbScroll.scanDomNode();
		this.crumbScroll.setScrollPosition({ scrollLeft: this.crumbScroll.getScrollDimensions().scrollWidth });
	}

	private updateButtons(): void {
		this.backButton.disabled = !this.back.length;
		this.forwardButton.disabled = !this.forward.length;
		this.upButton.disabled = !this.current?.path;
	}

	private goBack(): void {
		const target = this.back.pop();
		if (target && this.current) {
			this.forward.push(this.current);
			void this.navigate(target, false).then(() => {
				this.updateButtons();
			});
		}
	}

	private goForward(): void {
		const target = this.forward.pop();
		if (target && this.current) {
			this.back.push(this.current);
			void this.navigate(target, false).then(() => {
				this.updateButtons();
			});
		}
	}

	private goUp(): void {
		const target = this.current && parent(this.current);
		if (target) {
			void this.navigate(target);
		}
	}

	/** Opens a folder; on a transfer's row, does what it needs, such as answering its question. */
	private open(row: PaneRow): void {
		const entry = row.entry;
		if (entry?.kind === 'dir' && this.current) {
			void this.navigate(child(this.current, entry.name));
			return;
		}
		const arrival = row.arrival;
		if (!entry && arrival?.action && (row.decision || arrival.action !== 'cancel')) {
			this.requestTransfer({ jobId: arrival.jobId, action: arrival.action });
		} else if (entry && row.arrival?.decision) {
			this.requestTransfer({ jobId: row.arrival.jobId, action: 'choose' });
		}
	}

	/** A transfer's row offers what can be done about the transfer. */
	private showTransferMenu(anchor: HTMLElement | { x: number; y: number }, row: PaneRow): void {
		const arrival = row.arrival;
		if (!arrival) {
			return;
		}
		const labels = {
			choose: nls.localize('files.chooseAction', 'Choose…'),
			cancel: nls.localize('files.cancelTransferAction', 'Cancel Transfer'),
			resume: nls.localize('files.resume', 'Resume'),
			retry: nls.localize('files.tryAgainTransfer', 'Try Again'),
		};
		const actions: IAction[] = [];
		if (arrival.action) {
			const action = arrival.action;
			actions.push(toAction({
				id: `files.transfer.${action}`, label: labels[action], run: () => {
					this.requestTransfer({ jobId: arrival.jobId, action });
				}
			}));
		}
		if (arrival.action === 'choose' || arrival.action === 'resume' || arrival.action === 'retry') {
			actions.push(toAction({
				id: 'files.transfer.cancel', label: labels.cancel, run: () => {
					this.requestTransfer({ jobId: arrival.jobId, action: 'cancel' });
				}
			}));
		}
		if (actions.length) {
			this.contextMenuService.showContextMenu({ getAnchor: () => anchor, getActions: () => actions });
		}
	}

	/** The usual folders, then the disk and drives, read again each time so a drive plugged in since shows. */
	private async showPlaces(): Promise<void> {
		const disk = this.disk;
		if (!disk) {
			return;
		}
		const places = await disk.places().catch(() => this.places);
		if (!places || disk !== this.disk) {
			return;
		}
		this.places = places;
		const place = (folder: FilePlaces['folders'][number]) => toAction({
			id: `files.place.${folder.kind}${folder.kind === 'volume' ? `.${folder.path}` : ''}`,
			label: folderLabel(folder.kind, folder.path),
			checked: same(this.current, folder),
			run: () => void this.navigate({ root: folder.root, path: folder.path }),
		});
		const actions: IAction[] = places.folders.filter(folder => folder.kind !== 'volume').map(place);
		if (actions.length && places.roots.length) {
			actions.push(new Separator());
		}
		for (const root of places.roots) {
			actions.push(toAction({
				id: `files.root.${root.id}`,
				label: root.name === '/' ? nls.localize('files.wholeDisk', 'Whole Disk') : root.name,
				checked: same(this.current, { root: root.id, path: '' }),
				run: () => void this.navigate({ root: root.id, path: '' }),
			}));
		}
		actions.push(...places.folders.filter(folder => folder.kind === 'volume').map(place));
		this.contextMenuService.showContextMenu({
			getAnchor: () => this.nameButton,
			getActions: () => actions,
			autoSelectFirstItem: true,
			onHide: () => {
				this.nameButton.setAttribute('aria-expanded', 'false');
			},
		});
		this.nameButton.setAttribute('aria-expanded', 'true');
	}

	private showMenu(anchor: HTMLElement | { x: number; y: number }, entry: FileEntry | undefined): void {
		const selection = this.selectedEntries();
		const actions: IAction[] = [];
		if (entry) {
			if (entry.kind === 'dir' && selection.length === 1) {
				actions.push(toAction({
					id: 'files.open', label: nls.localize('files.openFolder', 'Open'), run: () => {
						this.open({ entry, arrival: undefined, decision: false });
					}
				}));
			}
			actions.push(toAction({
				id: 'files.copyAcross', label: nls.localize('files.copyAcross', 'Copy to the Other Side'), run: () => {
					this.requestedCopy.fire();
				}
			}));
			actions.push(new Separator());
			if (selection.length === 1) {
				actions.push(toAction({
					id: 'files.rename', label: nls.localize('files.rename', 'Rename'), run: () => {
						this.startRename();
					}
				}));
			}
			actions.push(toAction({ id: 'files.delete', label: this.trashLabel, run: () => void this.deleteSelection(false) }));
			actions.push(toAction({ id: 'files.deletePermanently', label: nls.localize('files.deletePermanently', 'Delete Permanently…'), run: () => void this.deleteSelection(true) }));
			actions.push(new Separator());
		}
		actions.push(toAction({ id: 'files.newFolder', label: nls.localize('files.newFolder', 'New Folder'), enabled: this.current !== undefined, run: () => void this.newFolder() }));
		actions.push(toAction({ id: 'files.refreshAction', label: nls.localize('files.refresh', 'Refresh'), run: () => void this.refresh() }));
		actions.push(toAction({
			id: 'files.hidden', label: nls.localize('files.showHidden', 'Show Hidden Files'), checked: this.showHidden, run: () => {
				this.toggleHidden();
			}
		}));
		this.contextMenuService.showContextMenu({
			getAnchor: () => anchor instanceof HTMLElement ? anchor : { x: anchor.x, y: anchor.y },
			getActions: () => actions,
		});
	}

	private toggleHidden(): void {
		this.showHidden = !this.showHidden;
		void this.uiState.set('files.showHidden', this.showHidden);
		this.show();
	}

	private startRename(): void {
		const entry = this.selectedEntries().at(0);
		const index = this.rows.findIndex(row => row.entry === entry);
		if (!entry || index < 0) {
			return;
		}
		this.editing = entry.name;
		this.list.splice(index, 1, [this.rows[index]]);
		this.list.reveal(index);
	}

	private async rename(entry: FileEntry, name: string): Promise<void> {
		const folder = this.current;
		if (!folder || !this.disk) {
			return;
		}
		let renamed = name;
		try {
			await this.disk.rename(folder, entry, name);
		} catch (error) {
			renamed = entry.name;
			this.say(engineErrorCategory(error) === 'InvalidSetting' && /stale|replaced/i.test(String(error)) ? nls.localize('files.changed', '“{0}” changed since it was listed. Look again and retry.', entry.name) : this.failureText(error));
		}
		await this.refresh();
		// The item keeps the selection under whichever name it now has.
		const index = this.rows.findIndex(row => row.entry?.name === renamed);
		if (index >= 0) {
			this.list.setSelection([index]);
			this.list.setFocus([index]);
			this.list.reveal(index);
		}
	}

	private async newFolder(): Promise<void> {
		const folder = this.current;
		if (!folder || !this.disk) {
			return;
		}
		const name = freshName(nls.localize('files.newFolderName', 'New Folder'), this.entries.map(entry => entry.name));
		try {
			await this.disk.mkdir(folder, name);
		} catch (error) {
			this.say(this.failureText(error));
			return;
		}
		await this.refresh();
		const index = this.rows.findIndex(row => row.entry?.name === name);
		if (index >= 0) {
			this.list.setSelection([index]);
			this.startRename();
		}
	}

	/** What the computer calls its Trash, and moving to it. */
	private get trashName(): string {
		return this.os === 'windows' ? nls.localize('files.recycleBin', 'Recycle Bin') : nls.localize('files.trash', 'Trash');
	}

	private get trashLabel(): string {
		return this.os === 'windows' ? nls.localize('files.deleteToBin', 'Delete') : nls.localize('files.moveToTrash', 'Move to Trash');
	}

	/**
	 * Deletes the selection: to the computer's Trash at once, as Finder and
	 * Explorer do, or `permanently` once asked. One question at a time: a
	 * second request while one is open, or a key repeating, asks nothing more.
	 */
	private async deleteSelection(permanently: boolean): Promise<void> {
		if (this.confirming) {
			return;
		}
		this.confirming = true;
		try {
			await this.deleteEntries(permanently);
		} finally {
			this.confirming = false;
		}
	}

	private async deleteEntries(permanently: boolean): Promise<void> {
		const folder = this.current;
		const disk = this.disk;
		const entries = this.selectedEntries();
		if (!folder || !disk || !entries.length || (permanently && !await this.confirmPermanent(entries, false))) {
			return;
		}
		let trashed = 0;
		for (const entry of entries) {
			try {
				await disk.delete(folder, entry, !permanently);
				trashed += permanently ? 0 : 1;
			} catch (refusal: unknown) {
				let error = refusal;
				// No Trash there: as Finder does, offer to delete it for good.
				if (!permanently && engineErrorCategory(error) === 'Unsupported' && await this.confirmPermanent([entry], true)) {
					try {
						await disk.delete(folder, entry, false);
						continue;
					} catch (again: unknown) {
						error = again;
					}
				}
				this.say(engineErrorCategory(error) === 'InvalidSetting' ? nls.localize('files.deleteFailed', '“{0}” couldn’t be deleted.', entry.name) : this.failureText(error));
				break;
			}
		}
		// The Trash on the other computer is out of sight: say where things went.
		if (trashed && this.side === 'remote') {
			this.say(trashed === 1
				? nls.localize('files.trashedOne', 'Moved “{0}” to the {1} on {2}.', entries[0].name, this.trashName, this.computer)
				: nls.localize('files.trashedMany', 'Moved {0} items to the {1} on {2}.', trashed, this.trashName, this.computer));
		}
		await this.refresh();
	}

	/** Asks before deleting for good; `noTrash` when the place has no Trash to move to. */
	private async confirmPermanent(entries: readonly FileEntry[], noTrash: boolean): Promise<boolean> {
		const { button } = await this.dialogService.show({
			type: 'warning',
			message: noTrash
				? nls.localize('files.noTrash', '“{0}” can’t be moved to the {1}. Delete it permanently?', entries[0].name, this.trashName)
				: entries.length === 1
					? nls.localize('files.deleteOne', 'Permanently delete “{0}” from {1}?', entries[0].name, this.computer)
					: nls.localize('files.deleteMany', 'Permanently delete {0} items from {1}?', entries.length, this.computer),
			detail: entries.some(entry => entry.kind === 'dir')
				? nls.localize('files.deleteFolderDetail', 'Folders are deleted with everything in them. This can’t be undone.')
				: nls.localize('files.deleteDetail', 'This can’t be undone.'),
			buttons: [nls.localize('files.deleteButton', 'Delete'), nls.localize('files.cancel', 'Cancel')],
			cancelId: 1,
		});
		return button === 0;
	}

	private onKey(event: StandardKeyboardEvent): void {
		const command = KeyMod.CtrlCmd;
		const handled = (): void => {
			event.preventDefault();
			event.stopPropagation();
		};
		const focused = this.list.getFocusedElements().at(0);
		if (event.equals(KeyCode.Enter) && focused) {
			handled();
			this.open(focused);
		} else if (event.equals(KeyCode.Home) || event.equals(KeyCode.End)) {
			handled();
			if (event.equals(KeyCode.Home)) {
				this.list.focusFirst(event.browserEvent);
			} else {
				this.list.focusLast(event.browserEvent);
			}
			const index = this.list.getFocus().at(0);
			if (index !== undefined) {
				this.list.reveal(index);
			}
		} else if (event.equals(KeyCode.F2)) {
			handled();
			this.startRename();
		} else if (event.equals(KeyCode.Backspace) && !isMacintosh || event.equals(command | KeyCode.UpArrow)) {
			handled();
			this.goUp();
		} else if (event.equals(command | KeyCode.BracketLeft) || event.equals(KeyMod.Alt | KeyCode.LeftArrow) && !isMacintosh) {
			handled();
			this.goBack();
		} else if (event.equals(command | KeyCode.BracketRight) || event.equals(KeyMod.Alt | KeyCode.RightArrow) && !isMacintosh) {
			handled();
			this.goForward();
		} else if (event.equals(command | (this.side === 'local' ? KeyCode.RightArrow : KeyCode.LeftArrow))) {
			handled();
			this.requestedCopy.fire();
		} else if (event.equals(KeyMod.Shift | KeyCode.Delete) || event.equals(command | KeyMod.Alt | KeyCode.Backspace)) {
			// Shift+Delete, as Explorer; Option-Command-Delete, as Finder.
			handled();
			void this.deleteSelection(true);
		} else if (event.equals(KeyCode.Delete) || event.equals(command | KeyCode.Backspace)) {
			handled();
			void this.deleteSelection(false);
		} else if (event.equals(command | KeyMod.Shift | KeyCode.KeyN)) {
			handled();
			void this.newFolder();
		} else if (event.equals(command | KeyCode.KeyR) || event.equals(KeyCode.F5)) {
			handled();
			void this.refresh();
		}
	}
}
