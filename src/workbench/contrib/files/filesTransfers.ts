import * as nls from '../../../nls.js';
import { $, addDisposableListener, append, EventType } from '../../../base/browser/dom.js';
import { renderIcon } from '../../../base/browser/ui/icon/icon.js';
import { Emitter } from '../../../base/common/event.js';
import { Icons } from '../../../base/common/icons.js';
import { Disposable, DisposableStore } from '../../../base/common/lifecycle.js';
import type { FileJob, FilePlaces } from '../../../platform/engine/common/files.js';
import { IDialogService } from '../../services/dialogs/dialogService.js';
import { isUnfinished, type IFilesSession } from '../../services/files/filesService.js';
import { folderOn, formatSize, jobStatus } from './filesText.js';

type Filter = 'all' | 'remote' | 'local';

/** How the panel names each side and where its folders are. */
export interface TransferSides {
	readonly local: { readonly computer: string; readonly places: FilePlaces | undefined };
	readonly remote: { readonly computer: string; readonly places: FilePlaces | undefined };
}

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

/**
 * The tab's transfers: a summary in the status bar, and a panel listing each
 * one with its progress and the one action that fits it.
 */
export class FilesTransfers extends Disposable {
	public readonly panel: HTMLElement;
	public readonly summary: HTMLButtonElement;
	public readonly decision: HTMLElement;
	private readonly rows: HTMLElement;
	private readonly tabs = new Map<Filter, HTMLButtonElement>();
	private readonly clear: HTMLButtonElement;
	private readonly rowListeners = this._register(new DisposableStore());
	private readonly samples = new Map<string, Sample>();
	private readonly toggled = this._register(new Emitter<boolean>());
	public readonly onDidToggle = this.toggled.event;
	private filter: Filter = 'all';
	private session: IFilesSession | undefined;
	private readonly sessionListeners = this._register(new DisposableStore());

	constructor(
		private readonly sides: () => TransferSides,
		@IDialogService private readonly dialogService: IDialogService,
	) {
		super();
		this.panel = $('section.files-transfers');
		this.panel.hidden = true;
		this.panel.setAttribute('aria-label', nls.localize('files.transfers', 'Transfers'));
		const header = append(this.panel, $('.files-transfers-header'));
		const tabList = append(header, $('.files-transfers-tabs', { role: 'tablist' }));
		for (const filter of ['all', 'remote', 'local'] as const) {
			const tab = append(tabList, $<HTMLButtonElement>('button.files-transfers-tab', { type: 'button', role: 'tab' }));
			this.tabs.set(filter, tab);
			this._register(addDisposableListener(tab, EventType.CLICK, () => {
				this.filter = filter;
				this.render();
			}));
		}
		append(header, $('span.files-transfers-space'));
		this.clear = append(header, $<HTMLButtonElement>('button.files-text-button', { type: 'button' }));
		this.clear.textContent = nls.localize('files.clearFinished', 'Clear Finished');
		const close = append(header, $<HTMLButtonElement>('button.files-icon-button', { type: 'button', title: nls.localize('files.closeTransfers', 'Close Transfers'), 'aria-label': nls.localize('files.closeTransfers', 'Close Transfers') }));
		append(close, renderIcon(Icons.close));
		this.rows = append(this.panel, $('.files-transfers-rows', { role: 'list' }));
		this.summary = $<HTMLButtonElement>('button.files-status-transfers', { type: 'button', 'aria-expanded': 'false' });
		this.summary.hidden = true;
		this.decision = $('span.files-status-decision');
		this.decision.hidden = true;
		this._register(addDisposableListener(this.summary, EventType.CLICK, () => {
			this.setOpen(this.panel.hidden !== false);
		}));
		this._register(addDisposableListener(close, EventType.CLICK, () => {
			this.setOpen(false);
		}));
		this._register(addDisposableListener(this.clear, EventType.CLICK, () => {
			void this.clearFinished();
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
		this.panel.hidden = !open;
		this.summary.setAttribute('aria-expanded', String(open));
		this.summary.classList.toggle('open', open);
		this.toggled.fire(open);
	}

	public render(): void {
		const jobs = this.session?.jobs ?? [];
		const now = performance.now();
		for (const job of jobs) {
			this.samples.set(job.id, sampleRate(this.samples.get(job.id), Number(job.done_bytes), now));
		}
		const { local, remote } = this.sides();
		for (const [filter, tab] of this.tabs) {
			const count = jobs.filter(job => this.matches(job, filter)).length;
			tab.textContent = filter === 'all'
				? nls.localize('files.allTransfers', 'All {0}', count)
				: nls.localize('files.toComputer', 'To {0} {1}', filter === 'remote' ? remote.computer : local.computer, count);
			tab.setAttribute('aria-selected', String(filter === this.filter));
		}
		this.clear.disabled = !jobs.some(job => !isUnfinished(job) && job.state !== 'Interrupted');
		this.rowListeners.clear();
		this.rows.replaceChildren();
		for (const job of jobs.filter(candidate => this.matches(candidate, this.filter))) {
			this.renderRow(job);
		}
		if (!this.rows.childElementCount) {
			append(this.rows, $('p.files-transfers-empty', undefined, nls.localize('files.noTransfers', 'Nothing is being copied.')));
		}
		this.renderSummary(jobs);
	}

	/** Whether `job` copies toward `filter`'s computer. Uploads go to the other computer. */
	private matches(job: FileJob, filter: Filter): boolean {
		return filter === 'all' || (filter === 'remote') === (job.direction === 'Upload');
	}

	private renderRow(job: FileJob): void {
		const { local, remote } = this.sides();
		const row = append(this.rows, $('.files-transfer', { role: 'listitem' }));
		row.dataset.state = job.state;
		append(row, $('span.files-transfer-direction', { 'aria-hidden': 'true' }, job.direction === 'Upload' ? '→' : '←'));
		const name = append(row, $('span.files-transfer-name'));
		name.textContent = job.label;
		name.title = job.label;
		const route = this.session?.routeOf(job.id);
		const to = job.direction === 'Upload' ? remote : local;
		const from = job.direction === 'Upload' ? local : remote;
		const routeText = route
			? nls.localize('files.route', '{0} → {1}', folderOn(route.source, from.places, from.computer), folderOn(route.destination, to.places, to.computer))
			: nls.localize('files.toOnly', 'To {0}', to.computer);
		append(row, $('span.files-transfer-route', { title: routeText }, routeText));
		const done = Number(job.done_bytes);
		const total = Number(job.total_bytes);
		const bar = append(row, $('span.files-transfer-bar', { role: 'progressbar', 'aria-valuemin': '0', 'aria-valuemax': '100' }));
		const percent = total > 0 ? Math.min(100, Math.round(done / total * 100)) : job.state === 'Complete' ? 100 : 0;
		bar.setAttribute('aria-valuenow', String(percent));
		bar.setAttribute('aria-label', nls.localize('files.progressLabel', '{0} copied', job.label));
		append(bar, $('span.files-transfer-fill')).style.width = `${String(percent)}%`;
		append(row, $('span.files-transfer-status', undefined, jobStatus(job, this.samples.get(job.id)?.rate)));
		const action = this.actionFor(job);
		const slot = append(row, $('span.files-transfer-action'));
		if (action) {
			const button = append(slot, $<HTMLButtonElement>('button.files-text-button', { type: 'button' }));
			button.textContent = action.label;
			button.classList.toggle('primary', action.primary);
			this.rowListeners.add(addDisposableListener(button, EventType.CLICK, () => {
				void action.run();
			}));
		}
	}

	private actionFor(job: FileJob): { readonly label: string; readonly primary: boolean; readonly run: () => Promise<void> } | undefined {
		const session = this.session;
		if (!session || session.ended) {
			return undefined;
		}
		switch (job.state) {
			case 'Preparing':
			case 'Serving':
			case 'Transferring':
				return { label: nls.localize('files.cancelTransfer', 'Cancel'), primary: false, run: () => session.cancel(job.id) };
			case 'WaitingDecision':
				return { label: nls.localize('files.choose', 'Choose…'), primary: true, run: () => this.choose(job) };
			case 'Interrupted':
			case 'Cancelled':
				return { label: nls.localize('files.resume', 'Resume'), primary: job.state === 'Interrupted', run: () => session.resume(job.id) };
			case 'Failed':
				return { label: nls.localize('files.tryAgainTransfer', 'Try Again'), primary: false, run: () => session.resume(job.id) };
			case 'Complete':
				return undefined;
		}
	}

	/** Asks what to do about a name already at the destination. */
	private async choose(job: FileJob): Promise<void> {
		const session = this.session;
		if (!session) {
			return;
		}
		const { local, remote } = this.sides();
		const to = job.direction === 'Upload' ? remote : local;
		const route = session.routeOf(job.id);
		const file = job.waiting_file?.slice(job.waiting_file.lastIndexOf('/') + 1) ?? job.label;
		const where = route ? folderOn(route.destination, to.places, to.computer) : to.computer;
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

	private renderSummary(jobs: readonly FileJob[]): void {
		const { remote } = this.sides();
		const active = jobs.filter(isUnfinished);
		const waiting = jobs.filter(job => job.state === 'WaitingDecision').length;
		this.summary.hidden = !jobs.length;
		this.decision.hidden = waiting === 0;
		this.decision.textContent = waiting === 1
			? nls.localize('files.oneDecision', '1 needs your decision')
			: nls.localize('files.decisions', '{0} need your decision', waiting);
		this.summary.replaceChildren(renderIcon(Icons.transfers));
		if (!active.length) {
			append(this.summary, $('span', undefined, nls.localize('files.transfersDone', 'Transfers')));
			return;
		}
		const done = active.reduce((sum, job) => sum + Number(job.done_bytes), 0);
		const total = active.reduce((sum, job) => sum + Number(job.total_bytes), 0);
		const rate = active.reduce((sum, job) => sum + (this.samples.get(job.id)?.rate ?? 0), 0);
		const toRemote = active.every(job => job.direction === 'Upload');
		const toLocal = active.every(job => job.direction === 'Download');
		const verb = toRemote
			? nls.localize('files.sendingTo', 'Copying to {0}', remote.computer)
			: toLocal ? nls.localize('files.receivingFrom', 'Copying from {0}', remote.computer) : nls.localize('files.copying', 'Copying');
		const amount = nls.localize('files.summaryAmount', '{0} of {1}', formatSize(done), formatSize(total));
		append(this.summary, $('span', undefined, rate > 0 ? nls.localize('files.summaryRate', '{0}: {1} · {2}/s', verb, amount, formatSize(rate)) : nls.localize('files.summary', '{0}: {1}', verb, amount)));
	}
}
