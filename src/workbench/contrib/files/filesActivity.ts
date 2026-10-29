import * as nls from '../../../nls.js';
import { $, addDisposableListener, append, EventType, getActiveElement } from '../../../base/browser/dom.js';
import { StandardKeyboardEvent } from '../../../base/browser/keyboardEvent.js';
import { renderIcon } from '../../../base/browser/ui/icon/icon.js';
import { Emitter } from '../../../base/common/event.js';
import { Icons, type IconId } from '../../../base/common/icons.js';
import { KeyCode } from '../../../base/common/keyCodes.js';
import { Disposable, DisposableStore, MutableDisposable, toDisposable } from '../../../base/common/lifecycle.js';
import type { FileConflictChoice, FileJob, FilePlaces } from '../../../platform/engine/common/files.js';
import type { FilesPane, FolderLocation } from '../../../platform/state/common/uiState.js';
import { IDialogService } from '../../services/dialogs/dialogService.js';
import { isUnfinished, type IFilesSession } from '../../services/files/filesService.js';
import { arrivalStatus, folderOn, formatSize, jobProgress, jobStatus } from './filesText.js';

/** How each side is named, and its folders. */
export interface ActivitySides {
	readonly local: { readonly computer: string; readonly places: FilePlaces | undefined };
	readonly remote: { readonly computer: string; readonly places: FilePlaces | undefined };
}

/** How a transfer's row reads at a glance: under way, waiting on you, stopped, failed, or just arrived. */
export type ArrivalTone = 'active' | 'pending' | 'stopped' | 'failed' | 'landed';

/** A transfer as the folder it lands in shows it, at the name it lands under. */
export interface FilesArrival {
	readonly jobId: string;
	readonly name: string;
	readonly folder: boolean;
	/** Its size, when the transfer is of it alone. */
	readonly bytes: number | undefined;
	readonly progress: number;
	readonly status: string;
	readonly tone: ArrivalTone;
	/** Set while a name already there needs an answer. */
	readonly decision: { readonly text: string; readonly many: boolean } | undefined;
	/** The single thing to do about it, as its row's menu and Enter offer it. */
	readonly action: TransferAction | undefined;
}

export type TransferAction = 'choose' | 'cancel' | 'resume' | 'retry';

/** Something done to a transfer from where it lands. */
export type TransferRequest =
	| { readonly jobId: string; readonly action: TransferAction }
	| { readonly jobId: string; readonly choice: FileConflictChoice; readonly applyToRemaining: boolean };

/** Where a transfer lands, to go and look at it. */
export interface TransferPlace {
	readonly side: FilesPane;
	readonly folder: FolderLocation;
	readonly name: string;
}

/** How long a transfer that just arrived keeps its tint. */
const LANDED_MS = 4000;

interface Sample {
	readonly bytes: number;
	readonly at: number;
	rate: number | undefined;
}

/** Bytes a second from the last two updates, smoothed so one late update doesn't make it jump. */
function sampleRate(previous: Sample | undefined, bytes: number, at: number): Sample {
	if (!previous || bytes < previous.bytes || at <= previous.at) {
		return { bytes, at, rate: undefined };
	}
	const instant = (bytes - previous.bytes) / ((at - previous.at) / 1000);
	return { bytes, at, rate: previous.rate === undefined ? instant : previous.rate * 0.7 + instant * 0.3 };
}

function toneOf(job: FileJob): ArrivalTone {
	switch (job.state) {
		case 'WaitingDecision': return 'pending';
		case 'Interrupted': return 'stopped';
		case 'Failed': return 'failed';
		case 'Complete': return 'landed';
		case 'Preparing': case 'Serving': case 'Transferring': case 'Cancelled': return 'active';
	}
}

function actionOf(job: FileJob): TransferAction | undefined {
	switch (job.state) {
		case 'Preparing': case 'Serving': case 'Transferring': return 'cancel';
		case 'WaitingDecision': return 'choose';
		case 'Interrupted': return 'resume';
		case 'Failed': return 'retry';
		case 'Cancelled': case 'Complete': return undefined;
	}
}

/**
 * A transfer's mark in the activity list: its ring while it still has bytes
 * to move, a check, error or cross once it has finished.
 */
function renderJobMark(job: FileJob): HTMLElement {
	const mark = (icon: IconId, tone: ArrivalTone | 'cancelled'): HTMLElement => {
		const element = renderIcon(icon);
		element.classList.add('files-mark');
		element.dataset.tone = tone;
		return element;
	};
	switch (job.state) {
		case 'Complete': return mark(Icons.check, 'landed');
		case 'Failed': return mark(Icons.error, 'failed');
		case 'Cancelled': return mark(Icons.close, 'cancelled');
		case 'Preparing': case 'Serving': case 'Transferring': case 'WaitingDecision': case 'Interrupted':
			return renderRing(jobProgress(job), toneOf(job));
	}
}

/** A ring that fills as a transfer goes: the arrival rows' and the activity list's progress. */
export function renderRing(progress: number, tone: ArrivalTone): HTMLElement {
	const ring = $('span.files-ring', { 'aria-hidden': 'true' });
	ring.dataset.tone = tone;
	ring.style.setProperty('--progress', String(progress));
	return ring;
}

/**
 * A Files tab's transfers. Each one shows where it lands, in the pane of
 * the receiving folder; this owns that view of them, the status bar items
 * that sum them up, and the activity list the status bar opens for those
 * out of view and the ones that have finished.
 */
export class FilesActivity extends Disposable {
	/** The status bar's summary, which opens and closes the list. */
	public readonly summary: HTMLButtonElement;
	/** The status bar's "needs your decision", which goes to the first one. */
	public readonly decision: HTMLButtonElement;
	/** The activity list, placed by the view above the status bar. */
	public readonly list: HTMLElement;
	private readonly title: HTMLElement;
	private readonly entries: HTMLElement;
	private readonly clear: HTMLButtonElement;
	private readonly entryListeners = this._register(new DisposableStore());
	private readonly sessionListeners = this._register(new DisposableStore());
	private readonly samples = new Map<string, Sample>();
	/** When each transfer was first seen complete, for its tint. */
	private readonly landedAt = new Map<string, number>();
	private readonly landedTimer = this._register(new MutableDisposable());
	private readonly changed = this._register(new Emitter<void>());
	/** Fires when what the panes show of the transfers changes. */
	public readonly onDidChange = this.changed.event;
	private readonly showRequested = this._register(new Emitter<TransferPlace>());
	/** Fires to go to where a transfer lands. */
	public readonly onDidRequestShow = this.showRequested.event;
	private session: IFilesSession | undefined;

	constructor(
		private readonly sides: () => ActivitySides,
		@IDialogService private readonly dialogService: IDialogService,
	) {
		super();
		this.summary = $<HTMLButtonElement>('button.files-status-transfers', { type: 'button', 'aria-expanded': 'false', 'aria-haspopup': 'dialog' });
		this.summary.hidden = true;
		this.decision = $<HTMLButtonElement>('button.files-status-decision', { type: 'button' });
		this.decision.hidden = true;
		this.list = $('section.files-activity', { role: 'dialog', tabindex: '-1' });
		this.list.hidden = true;
		const header = append(this.list, $('.files-activity-header'));
		this.title = append(header, $('h2.files-activity-title'));
		this.clear = append(header, $<HTMLButtonElement>('button.files-text-button', { type: 'button' }));
		this.clear.textContent = nls.localize('files.clearFinished', 'Clear Finished');
		const close = append(header, $<HTMLButtonElement>('button.files-icon-button', { type: 'button', title: nls.localize('files.closeTransfers', 'Close'), 'aria-label': nls.localize('files.closeTransfers', 'Close') }));
		append(close, renderIcon(Icons.close));
		this.entries = append(this.list, $('.files-activity-entries'));
		this._register(addDisposableListener(this.summary, EventType.CLICK, () => {
			this.setOpen(this.list.hidden === true);
		}));
		this._register(addDisposableListener(this.decision, EventType.CLICK, () => {
			const waiting = this.session?.jobs.find(job => job.state === 'WaitingDecision');
			if (waiting) {
				this.show(waiting);
			}
		}));
		this._register(addDisposableListener(close, EventType.CLICK, () => {
			this.setOpen(false);
		}));
		this._register(addDisposableListener(this.clear, EventType.CLICK, () => {
			void this.clearFinished();
		}));
		// Escape closes the list and gives focus back to what opened it.
		this._register(addDisposableListener(this.list, EventType.KEY_DOWN, event => {
			if (new StandardKeyboardEvent(event).equals(KeyCode.Escape)) {
				event.preventDefault();
				event.stopPropagation();
				this.setOpen(false);
			}
		}));
		this._register(toDisposable(() => {
			this.landedTimer.clear();
		}));
	}

	public setSession(session: IFilesSession | undefined): void {
		this.sessionListeners.clear();
		this.session = session;
		if (session) {
			this.sessionListeners.add(session.onDidChangeJobs(() => {
				this.render();
			}));
		}
		this.render();
	}

	public setOpen(open: boolean): void {
		// Asked before hiding it, which moves focus out of it.
		const hadFocus = !this.list.hidden && this.list.contains(getActiveElement());
		this.list.hidden = !open;
		this.summary.setAttribute('aria-expanded', String(open));
		this.summary.classList.toggle('open', open);
		if (open) {
			this.render();
			this.list.focus();
		} else if (hadFocus) {
			this.summary.focus();
		}
	}

	/** The transfers landing in `folder` on `side`, by the name each lands under. */
	public arrivals(side: FilesPane, folder: FolderLocation | undefined): FilesArrival[] {
		const session = this.session;
		if (!session || !folder) {
			return [];
		}
		const { local, remote } = this.sides();
		const from = side === 'local' ? remote.computer : local.computer;
		const now = performance.now();
		const arrivals: FilesArrival[] = [];
		for (const job of session.jobs) {
			const destination = job.destination;
			if (!destination || (job.direction === 'Upload') !== (side === 'remote') || destination.root !== folder.root || destination.path !== folder.path) {
				continue;
			}
			// Finished ones look like any other row, once their tint has faded.
			if (job.state === 'Cancelled' || (job.state === 'Complete' && now - (this.landedAt.get(job.id) ?? 0) > LANDED_MS)) {
				continue;
			}
			const waiting = job.waiting_file;
			const many = Number(job.files_total) > 1;
			// The question sits at the item the waiting file belongs to.
			const asking = waiting?.split('/')[0] ?? destination.items.at(0)?.name;
			for (const item of destination.items) {
				arrivals.push({
					jobId: job.id,
					name: item.name,
					// Until the sender has listed what it sends, a transfer of many files is a folder.
					folder: item.folder || (many && destination.items.length === 1),
					bytes: destination.items.length === 1 ? Number(job.total_bytes) : undefined,
					progress: jobProgress(job),
					status: arrivalStatus(job, this.samples.get(job.id)?.rate),
					tone: toneOf(job),
					action: actionOf(job),
					decision: job.state === 'WaitingDecision' && item.name === asking
						? {
							text: nls.localize('files.sendingAnother', '{0} is sending another “{1}”', from, waiting ? waiting.slice(waiting.lastIndexOf('/') + 1) : item.name),
							many,
						}
						: undefined,
				});
			}
		}
		return arrivals;
	}

	/** Acts on a transfer from where it lands. */
	public async act(request: TransferRequest): Promise<void> {
		const session = this.session;
		const job = session?.jobs.find(candidate => candidate.id === request.jobId);
		if (!session || session.ended || !job) {
			return;
		}
		try {
			if ('choice' in request) {
				await session.resolve(job.id, request.choice, request.applyToRemaining);
				return;
			}
			switch (request.action) {
				case 'choose': await this.choose(job); break;
				case 'cancel': await session.cancel(job.id); break;
				case 'resume': case 'retry': await session.resume(job.id); break;
			}
		} catch {
			// The transfer's own state says what happened; a lost connection shows in the pane.
		}
	}

	public render(): void {
		const jobs = this.session?.jobs ?? [];
		const now = performance.now();
		for (const job of jobs) {
			this.samples.set(job.id, sampleRate(this.samples.get(job.id), Number(job.done_bytes), now));
			if (job.state === 'Complete' && !this.landedAt.has(job.id)) {
				this.landedAt.set(job.id, now);
				this.scheduleFade();
			}
		}
		this.renderSummary(jobs);
		if (!this.list.hidden) {
			this.renderList(jobs);
		}
		this.changed.fire();
	}

	/** Lets the panes drop the tint of what arrived once it has had its moment. */
	private scheduleFade(): void {
		const timer = setTimeout(() => {
			this.landedTimer.clear();
			this.changed.fire();
		}, LANDED_MS + 50);
		this.landedTimer.value = toDisposable(() => {
			clearTimeout(timer);
		});
	}

	/** Where a transfer lands, at the item it asks about or else its first. */
	private placeOf(job: FileJob): TransferPlace | undefined {
		const destination = job.destination;
		const name = job.waiting_file?.split('/')[0] ?? destination?.items.at(0)?.name;
		return destination && name !== undefined ? { side: job.direction === 'Upload' ? 'remote' : 'local', folder: { root: destination.root, path: destination.path }, name } : undefined;
	}

	private show(job: FileJob): void {
		const place = this.placeOf(job);
		if (place) {
			this.setOpen(false);
			this.showRequested.fire(place);
		}
	}

	private renderSummary(jobs: readonly FileJob[]): void {
		const { remote } = this.sides();
		const active = jobs.filter(isUnfinished).filter(job => job.state !== 'WaitingDecision');
		const stopped = jobs.filter(job => job.state === 'Interrupted').length;
		const waiting = jobs.filter(job => job.state === 'WaitingDecision').length;
		this.summary.hidden = !jobs.length;
		this.decision.hidden = waiting === 0;
		this.decision.textContent = waiting === 1
			? nls.localize('files.oneDecision', '1 needs your decision')
			: nls.localize('files.decisions', '{0} need your decision', waiting);
		this.summary.replaceChildren();
		if (!active.length) {
			append(this.summary, renderIcon(Icons.transfers));
			append(this.summary, $('span', undefined, stopped
				? stopped === 1 ? nls.localize('files.oneStopped', '1 stopped') : nls.localize('files.manyStopped', '{0} stopped', stopped)
				: nls.localize('files.transfersDone', 'Transfers')));
			this.summary.classList.toggle('stopped', stopped > 0);
			return;
		}
		this.summary.classList.remove('stopped');
		const done = active.reduce((sum, job) => sum + Number(job.done_bytes), 0);
		const total = active.reduce((sum, job) => sum + Number(job.total_bytes), 0);
		const rate = active.reduce((sum, job) => sum + (this.samples.get(job.id)?.rate ?? 0), 0);
		append(this.summary, renderRing(total > 0 ? done / total : 0, 'active'));
		const toRemote = active.every(job => job.direction === 'Upload');
		const toLocal = active.every(job => job.direction === 'Download');
		const verb = toRemote
			? nls.localize('files.sendingTo', 'Copying to {0}', remote.computer)
			: toLocal ? nls.localize('files.receivingFrom', 'Copying from {0}', remote.computer) : nls.localize('files.copying', 'Copying');
		const amount = nls.localize('files.summaryAmount', '{0} of {1}', formatSize(done), formatSize(total));
		append(this.summary, $('span', undefined, rate > 0 ? nls.localize('files.summaryRate', '{0}: {1} · {2}/s', verb, amount, formatSize(rate)) : nls.localize('files.summary', '{0}: {1}', verb, amount)));
	}

	private renderList(jobs: readonly FileJob[]): void {
		const { local, remote } = this.sides();
		this.title.textContent = nls.localize('files.activityTitle', 'Transfers with {0}', remote.computer);
		this.list.setAttribute('aria-label', this.title.textContent);
		this.clear.disabled = !jobs.some(job => job.state === 'Complete' || job.state === 'Cancelled' || job.state === 'Failed');
		this.entryListeners.clear();
		this.entries.replaceChildren();
		const now = jobs.filter(job => isUnfinished(job) || job.state === 'Interrupted');
		const earlier = jobs.filter(job => !now.includes(job));
		if (!jobs.length) {
			append(this.entries, $('p.files-activity-empty', undefined, nls.localize('files.noTransfers', 'Nothing has been copied yet.')));
		}
		for (const [label, group] of [[nls.localize('files.now', 'Now'), now], [nls.localize('files.earlier', 'Earlier'), earlier]] as const) {
			if (!group.length) {
				continue;
			}
			const section = append(this.entries, $('section.files-activity-group'));
			append(section, $('h3.files-activity-heading', undefined, label));
			const items = append(section, $('ul.files-activity-items'));
			items.setAttribute('aria-label', label);
			for (const job of [...group].reverse()) {
				const to = job.direction === 'Upload' ? remote : local;
				const destination = job.destination;
				const where = destination ? folderOn({ root: destination.root, path: destination.path }, to.places, to.computer) : to.computer;
				const item = append(items, $('li.files-activity-item'));
				item.dataset.state = job.state;
				append(item, renderJobMark(job));
				const text = append(item, $('span.files-activity-text'));
				append(text, $('span.files-activity-name', { title: job.label }, job.label));
				const detail = job.state === 'Complete'
					? nls.localize('files.inFolder', 'In {0} · {1}', where, jobStatus(job, undefined))
					: nls.localize('files.toFolder', 'To {0} · {1}', where, jobStatus(job, this.samples.get(job.id)?.rate));
				append(text, $('span.files-activity-detail', { title: detail }, detail));
				const action = this.listAction(job);
				if (action) {
					const button = append(item, $<HTMLButtonElement>('button.files-text-button', { type: 'button' }));
					button.textContent = action.label;
					button.classList.toggle('primary', action.primary);
					this.entryListeners.add(addDisposableListener(button, EventType.CLICK, () => {
						action.run();
					}));
				}
			}
		}
	}

	private listAction(job: FileJob): { readonly label: string; readonly primary: boolean; readonly run: () => void } | undefined {
		const place = this.placeOf(job);
		switch (job.state) {
			case 'Preparing': case 'Serving': case 'Transferring':
				return { label: nls.localize('files.cancelTransfer', 'Cancel'), primary: false, run: () => void this.act({ jobId: job.id, action: 'cancel' }) };
			case 'WaitingDecision':
				return place
					? {
						label: nls.localize('files.showDecision', 'Show'), primary: true, run: () => {
							this.show(job);
						}
					}
					: { label: nls.localize('files.choose', 'Choose…'), primary: true, run: () => void this.act({ jobId: job.id, action: 'choose' }) };
			case 'Interrupted':
				return { label: nls.localize('files.resume', 'Resume'), primary: true, run: () => void this.act({ jobId: job.id, action: 'resume' }) };
			case 'Failed':
				return { label: nls.localize('files.tryAgainTransfer', 'Try Again'), primary: false, run: () => void this.act({ jobId: job.id, action: 'retry' }) };
			case 'Complete':
				return place ? {
					label: nls.localize('files.showArrived', 'Show'), primary: false, run: () => {
						this.show(job);
					}
				} : undefined;
			case 'Cancelled':
				return undefined;
		}
	}

	/** Asks what to do about a name already there, for the keyboard and the row's menu. */
	private async choose(job: FileJob): Promise<void> {
		const session = this.session;
		if (!session) {
			return;
		}
		const { local, remote } = this.sides();
		const to = job.direction === 'Upload' ? remote : local;
		const destination = job.destination;
		const file = job.waiting_file?.slice(job.waiting_file.lastIndexOf('/') + 1) ?? job.label;
		const where = destination ? folderOn({ root: destination.root, path: destination.path }, to.places, to.computer) : to.computer;
		const choices = ['skip', 'replace', 'keepBoth'] as const;
		const { button, checked } = await this.dialogService.show({
			type: 'warning',
			message: nls.localize('files.conflict', '{0} already has “{1}”', where, file),
			detail: nls.localize('files.conflictDetail', 'Keep Both names the copy with a number. Replace keeps the file there until the new one has fully arrived.'),
			buttons: [nls.localize('files.skip', 'Skip'), nls.localize('files.replace', 'Replace'), nls.localize('files.keepBoth', 'Keep Both'), nls.localize('files.cancel', 'Cancel')],
			cancelId: 3,
			checkbox: Number(job.files_total) > 1 ? { label: nls.localize('files.sameForOthers', 'Same answer for the other names that clash') } : undefined,
		});
		const choice = choices.at(button);
		if (choice && button < choices.length) {
			await session.resolve(job.id, choice, checked);
		}
	}

	private async clearFinished(): Promise<void> {
		const session = this.session;
		if (!session) {
			return;
		}
		for (const job of session.jobs.filter(candidate => candidate.state === 'Complete' || candidate.state === 'Cancelled' || candidate.state === 'Failed')) {
			await session.forget(job.id).catch(() => undefined);
		}
	}
}
