import * as nls from '../../../nls.js';
import { $, addDisposableListener, append, EventType } from '../../../base/browser/dom.js';
import { renderIcon } from '../../../base/browser/ui/icon/icon.js';
import { Icons } from '../../../base/common/icons.js';
import { Disposable, DisposableStore, MutableDisposable, toDisposable } from '../../../base/common/lifecycle.js';
import { isMacintosh, isWindows } from '../../../base/common/platform.js';
import { IInstantiationService } from '../../../platform/instantiation/common/instantiation.js';
import type { FilesPane, FolderLocation } from '../../../platform/state/common/uiState.js';
import type { DeviceSummary } from '../../services/devices/deviceService.js';
import { IFilesService, type FilesFailure, type FilesItem, type IFilesSession } from '../../services/files/filesService.js';
import type { WorkbenchTab } from '../../services/tabs/tabService.js';
import { FilesPaneView } from './filesPane.js';
import { FilesActivity, type TransferPlace } from './filesActivity.js';
import { folderName, formatSize } from './filesText.js';

/** This computer, as file managers call it. */
function thisComputer(): string {
	return isMacintosh ? nls.localize('files.thisMac', 'This Mac') : isWindows ? nls.localize('files.thisPC', 'This PC') : nls.localize('files.thisComputer', 'This Computer');
}

/**
 * Files with one paired computer: this computer's disk and the other's side
 * by side, a copy button between them that names what it will copy and
 * where. A transfer shows in the folder it lands in; the status bar sums
 * them up and opens the list of them all. One tab per computer.
 */
export class FilesView extends Disposable implements WorkbenchTab {
	public static readonly PREFIX = 'files:';
	public readonly id: string;
	public readonly label: string;
	public readonly icon = Icons.files;
	public readonly element: HTMLElement;
	private readonly local: FilesPaneView;
	private readonly remote: FilesPaneView;
	private readonly copyButton: HTMLButtonElement;
	private readonly copyText: HTMLElement;
	private readonly activity: FilesActivity;
	private readonly selection: HTMLElement;
	private readonly sessionListeners = this._register(new DisposableStore());
	private session: IFilesSession | undefined;
	private active: FilesPane = 'local';
	private opening = 0;
	/** Reconnecting after the connection was lost: attempts so far, and the next one. */
	private retries = 0;
	private readonly retry = this._register(new MutableDisposable());
	/** Each transfer's last state and files done, to see what newly landed. */
	private readonly landed = new Map<string, { readonly state: string; readonly files: number }>();
	private readonly refreshTimers = new Map<FilesPane, ReturnType<typeof setTimeout>>();
	private readonly refreshAgain = new Set<FilesPane>();

	constructor(
		private readonly device: DeviceSummary,
		@IFilesService private readonly filesService: IFilesService,
		@IInstantiationService instantiationService: IInstantiationService,
	) {
		super();
		this.id = FilesView.idFor(device.id);
		this.label = device.name;
		this.element = $('section.files-view');
		this.element.setAttribute('role', 'region');
		this.element.setAttribute('aria-label', nls.localize('files.region', 'Files on this computer and {0}', device.name));
		const panes = append(this.element, $('.files-panes'));
		this.local = this._register(instantiationService.createInstance(FilesPaneView, 'local', device.id, thisComputer(), isMacintosh ? 'macos' : isWindows ? 'windows' : 'linux'));
		this.remote = this._register(instantiationService.createInstance(FilesPaneView, 'remote', device.id, device.name, device.os ?? 'other'));
		const column = $('.files-copy');
		this.copyButton = append(column, $<HTMLButtonElement>('button.files-copy-button', { type: 'button' }));
		this.copyText = append(column, $('p.files-copy-text', { 'aria-hidden': 'true' }));
		append(panes, this.local.element, column, this.remote.element);
		this.activity = this._register(instantiationService.createInstance(FilesActivity, () => ({
			local: { computer: this.local.computer, places: this.local.knownPlaces },
			remote: { computer: this.remote.computer, places: this.remote.knownPlaces },
		})));
		const status = append(this.element, $('footer.files-status'));
		this.selection = append(status, $('span.files-status-selection', { role: 'status' }));
		append(status, $('span.files-status-space'), this.activity.decision, this.activity.summary);
		append(this.element, this.activity.list);
		this._register(this.activity.onDidChange(() => {
			this.showArrivals();
		}));
		this._register(this.activity.onDidRequestShow(place => {
			void this.showPlace(place);
		}));
		for (const pane of [this.local, this.remote]) {
			this._register(pane.onDidActivate(() => {
				this.activate(pane.side);
			}));
			this._register(pane.onDidChangeSelection(() => {
				this.update();
			}));
			this._register(pane.onDidNavigate(() => {
				pane.setArrivals(this.activity.arrivals(pane.side, pane.folder));
				this.update();
			}));
			this._register(pane.onDidRequestTransfer(request => {
				void this.activity.act(request);
			}));
			this._register(pane.onDidRequestCopy(() => {
				this.activate(pane.side);
				void this.copy();
			}));
			this._register(pane.onDidDrop(({ items, into }) => {
				void this.copyItems(pane.side === 'local' ? 'remote' : 'local', items, into);
			}));
		}
		this._register(addDisposableListener(this.copyButton, EventType.CLICK, () => {
			void this.copy();
		}));
		this._register({
			dispose: () => {
				this.session?.release();
				for (const timer of this.refreshTimers.values()) {
					clearTimeout(timer);
				}
			},
		});
		this.activate('local');
		void this.local.start(filesService.local);
		void this.connect();
	}

	public static idFor(deviceId: string): string {
		return `${FilesView.PREFIX}${deviceId}`;
	}

	/**
	 * Opens the other computer's side, or says in its pane why it can't.
	 * `again` is an attempt after the connection was lost; a lost
	 * connection is tried again, sooner first, while the tab is open.
	 */
	private async connect(again = false): Promise<void> {
		const attempt = ++this.opening;
		if (!again) {
			this.retries = 0;
			this.retry.clear();
		}
		this.remote.setUnavailable(again ? nls.localize('files.reconnecting', 'Reconnecting to {0}…', this.device.name) : nls.localize('files.connecting', 'Opening {0}…', this.device.name), '');
		const result = await this.filesService.open(this.device.id);
		if (attempt !== this.opening || this._store.isDisposed) {
			if ('session' in result) {
				result.session.release();
			}
			return;
		}
		if ('failure' in result) {
			this.showFailure(result.failure);
			if (again && result.failure === 'disconnected') {
				this.scheduleRetry();
			}
			return;
		}
		this.retries = 0;
		this.adopt(result.session);
		void this.remote.start(result.session.remote);
	}

	/** Tries the connection again: after 1, 2, 5 and 10 seconds, then every 30. */
	private scheduleRetry(): void {
		const delays = [1000, 2000, 5000, 10000];
		const delay = delays.at(this.retries) ?? 30000;
		this.retries++;
		const timer = setTimeout(() => {
			void this.connect(true);
		}, delay);
		this.retry.value = toDisposable(() => {
			clearTimeout(timer);
		});
	}

	private adopt(session: IFilesSession): void {
		this.sessionListeners.clear();
		this.session?.release();
		this.session = session;
		this.activity.setSession(session);
		this.landed.clear();
		for (const job of session.jobs) {
			this.landed.set(job.id, { state: job.state, files: Number(job.files_done) });
		}
		this.sessionListeners.add(session.onDidChangeJobs(() => {
			this.refreshDestinations(session);
		}));
		this.sessionListeners.add(session.onDidEnd(() => {
			this.showFailure('disconnected');
			this.scheduleRetry();
		}));
		this.update();
	}

	/**
	 * Shows what a transfer brought: the pane on the receiving side lists
	 * its folder again as files land and when the transfer completes, while
	 * it shows that folder or one inside it.
	 */
	private refreshDestinations(session: IFilesSession): void {
		for (const job of session.jobs) {
			const before = this.landed.get(job.id);
			const now = { state: job.state, files: Number(job.files_done) };
			this.landed.set(job.id, now);
			const arrived = before !== undefined && (now.files > before.files || (now.state === 'Complete' && before.state !== 'Complete'));
			if (!arrived) {
				continue;
			}
			const pane = job.direction === 'Upload' ? this.remote : this.local;
			const destination = job.destination;
			const folder = pane.folder;
			if (destination && folder && (folder.root !== destination.root || !(folder.path === destination.path || folder.path.startsWith(destination.path ? `${destination.path}/` : '')))) {
				continue;
			}
			this.scheduleRefresh(pane);
		}
	}

	/** Each pane shows the transfers landing in its folder. */
	private showArrivals(): void {
		for (const pane of [this.local, this.remote]) {
			pane.setArrivals(this.activity.arrivals(pane.side, pane.folder));
		}
	}

	/** Opens the folder a transfer lands in, at its row. */
	private async showPlace(place: TransferPlace): Promise<void> {
		const pane = place.side === 'local' ? this.local : this.remote;
		const folder = pane.folder;
		const there = folder?.root === place.folder.root && folder.path === place.folder.path;
		if (!there && !await pane.navigate(place.folder)) {
			return;
		}
		this.activate(place.side);
		pane.reveal(place.name);
	}

	/** Lists `pane`'s folder again now, then at most once a second, with a last listing for what landed in between. */
	private scheduleRefresh(pane: FilesPaneView): void {
		if (this.refreshTimers.has(pane.side)) {
			this.refreshAgain.add(pane.side);
			return;
		}
		void pane.refresh();
		this.refreshTimers.set(pane.side, setTimeout(() => {
			this.refreshTimers.delete(pane.side);
			if (this.refreshAgain.delete(pane.side)) {
				this.scheduleRefresh(pane);
			}
		}, 1000));
	}

	private showFailure(failure: FilesFailure): void {
		const name = this.device.name;
		const reconnect = { label: nls.localize('files.reconnect', 'Reconnect'), run: () => void this.connect() };
		switch (failure) {
			case 'turnedOff':
				this.remote.setUnavailable(nls.localize('files.offTitle', '{0} doesn’t share its files.', name), nls.localize('files.offBody', 'Someone at {0} can turn on Files in VniLink › Access & Security.', name), reconnect);
				break;
			case 'needsPermission':
				this.remote.setUnavailable(nls.localize('files.permissionTitle', '{0} hasn’t allowed VniLink to read its files.', name), nls.localize('files.permissionBody', 'Someone at {0} can allow it in the system’s privacy settings.', name), reconnect);
				break;
			case 'unsupported':
				this.remote.setUnavailable(nls.localize('files.unsupported', '{0} can’t share files.', name), nls.localize('files.unsupportedBody', 'Its version of VniLink doesn’t offer Files.'));
				break;
			case 'unavailable':
				this.remote.setUnavailable(nls.localize('files.unavailable', 'Files isn’t available on {0} right now.', name), '', reconnect);
				break;
			case 'disconnected':
				this.remote.setUnavailable(nls.localize('files.disconnected', 'Not connected to {0}.', name), nls.localize('files.disconnectedBody', 'Transfers keep what has arrived and can resume once it’s back.'), reconnect);
				break;
			case 'failed':
				this.remote.setUnavailable(nls.localize('files.openFailed', '{0}’s files couldn’t be opened.', name), '', reconnect);
				break;
		}
		this.update();
	}

	private activate(side: FilesPane): void {
		this.active = side;
		this.local.setActive(side === 'local');
		this.remote.setActive(side === 'remote');
		this.update();
	}

	private async copy(): Promise<void> {
		const source = this.active === 'local' ? this.local : this.remote;
		const target = this.active === 'local' ? this.remote : this.local;
		const items = source.selectedItems();
		if (items.length && target.folder) {
			await this.copyItems(this.active, items, target.folder);
		}
	}

	private async copyItems(from: FilesPane, items: readonly FilesItem[], into: FolderLocation): Promise<void> {
		const session = this.session;
		if (!session || session.ended || !items.length) {
			return;
		}
		try {
			await session.copy(from, items, into);
		} catch {
			// The status bar's list shows what was refused; a lost connection shows in the pane.
		}
	}

	/** The copy button, the status bar's selection line, and the transfers. */
	private update(): void {
		const source = this.active === 'local' ? this.local : this.remote;
		const target = this.active === 'local' ? this.remote : this.local;
		const { shown, selected, bytes, truncated } = source.summary();
		const into = target.folder;
		const usable = selected > 0 && into !== undefined && this.session !== undefined && !this.session.ended;
		const where = into ? nls.localize('files.copyWhere', '{0} on {1}', folderName(into, target.knownPlaces, target.computer), target.computer) : target.computer;
		this.copyButton.disabled = !usable;
		this.copyButton.replaceChildren(renderIcon(this.active === 'local' ? Icons.forward : Icons.back));
		this.copyButton.classList.toggle('toward-local', this.active === 'remote');
		const shortcut = isMacintosh ? (this.active === 'local' ? '⌘→' : '⌘←') : (this.active === 'local' ? 'Ctrl+→' : 'Ctrl+←');
		const label = usable
			? nls.localize('files.copyLabel', 'Copy {0} to {1}', selected, where)
			: nls.localize('files.copyNothing', 'Select files or folders to copy them');
		this.copyButton.setAttribute('aria-label', label);
		this.copyButton.title = `${label} (${shortcut})`;
		this.copyText.replaceChildren(
			$('span', undefined, usable ? nls.localize('files.copyCount', 'Copy {0} to', selected) : nls.localize('files.selectToCopy', 'Select files')),
			$('span', undefined, usable ? where : nls.localize('files.toCopyThem', 'to copy them')),
		);
		this.selection.textContent = selected
			? bytes > 0
				? nls.localize('files.selectedSummary', '{0}: {1} of {2} selected, {3}', source.computer, selected, shown, formatSize(bytes))
				: nls.localize('files.selectedFolders', '{0}: {1} of {2} selected', source.computer, selected, shown)
			: truncated
				? nls.localize('files.truncatedSummary', '{0}: showing the first {1} items', source.computer, shown)
				: nls.localize('files.shownSummary', '{0}: {1} items', source.computer, shown);
	}
}
