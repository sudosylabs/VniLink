import { DesktopCodecSection } from './desktopCodecSection.js';
import { wrapSelect } from './settingsControls.js';
import { IInstantiationService } from '../../../../../platform/instantiation/common/instantiation.js';
import { DesktopDefaultsSection } from './desktopDefaultsSection.js';
import * as nls from '../../../../../nls.js';
import { Disposable } from '../../../../../base/common/lifecycle.js';
import { addDisposableListener } from '../../../../../base/browser/domEvent.js';
import type { TabAction } from '../../../../services/tabs/common/tabService.js';
import { createPageHeading, type WorkbenchPage } from '../../../parts/views/page.js';
import { IRemoteDesktopService, DesktopReadbackError, desktopTargetKey, type DesktopSession, type DesktopSettings } from '../../../../services/remoteDesktop/common/remoteDesktopService.js';
import { DesktopQualityEditor } from './desktopQualityEditor.js';
import './media/remoteDesktop.css';

export class RemoteDesktopPage extends Disposable implements WorkbenchPage {
	public readonly element = document.createElement('main');
	public readonly actions: readonly TabAction[] = [];
	private readonly content = document.createElement('div');
	private readonly selector = document.createElement('select');
	private readonly refresh = document.createElement('button');
	private readonly apply = document.createElement('button');
	private readonly status = document.createElement('p');
	private readonly empty = document.createElement('p');
	private readonly displays = document.createElement('div');
	private readonly codec = document.createElement('p');
	private readonly sessionForm = document.createElement('form');
	private readonly editor = this._register(new DesktopQualityEditor());
	private settings: DesktopSettings | undefined;
	private selected: DesktopSession | undefined;
	private busy = false;
	private revision = 0;
	constructor(@IRemoteDesktopService private readonly desktop: IRemoteDesktopService, @IInstantiationService instantiation: IInstantiationService) {
		super(); this.element.className = 'remote-desktop-page'; this.element.dataset.settingsPage = 'desktop';
		this.element.append(createPageHeading(nls.localize('home.desktop', 'Remote Desktop'), nls.localize('desktop.description', 'Default stream quality and settings for active connections.')));
		this.content.className = 'desktop-content'; const session = document.createElement('section'); session.className = 'desktop-session-section';
		const header = document.createElement('div'); header.className = 'settings-section-header';
		const heading = document.createElement('h2'); heading.textContent = nls.localize('desktop.session', 'Session overrides');
		this.refresh.type = 'button'; this.refresh.className = 'settings-text-button'; this.refresh.textContent = nls.localize('desktop.refresh', 'Refresh'); header.append(heading, this.refresh);
		const label = document.createElement('label'); label.className = 'settings-control-row'; const text = document.createElement('span'); text.textContent = nls.localize('desktop.connection', 'Connection'); label.append(text, wrapSelect(this.selector));
		this.empty.className = 'settings-help'; session.append(header, label, this.empty);
		const form = this.sessionForm; form.hidden = true; form.noValidate = true; form.append(this.editor.element);
		const footer = document.createElement('div'); footer.className = 'settings-apply-row'; this.apply.type = 'submit'; this.apply.className = 'settings-button primary'; this.apply.textContent = nls.localize('desktop.apply', 'Apply to session');
		const help = document.createElement('p'); help.className = 'settings-help'; help.textContent = nls.localize('desktop.applyHelp', 'Limits apply to this session. Reconnecting uses your saved defaults.'); footer.append(this.apply, help);
		this.status.className = 'settings-status'; this.status.setAttribute('role', 'status'); form.append(footer);
		const displays = document.createElement('section'); displays.className = 'desktop-display-section'; const displaysTitle = document.createElement('h2'); displaysTitle.textContent = nls.localize('desktop.displays', 'Displays'); this.displays.className = 'desktop-display-list'; displays.append(displaysTitle, this.displays);
		const processing = document.createElement('section'); processing.className = 'desktop-processing-section'; const codecTitle = document.createElement('h2'); codecTitle.textContent = nls.localize('desktop.processing', 'Video processing on this computer'); this.codec.className = 'settings-help'; processing.append(codecTitle, this.codec);
		this.content.append(this._register(instantiation.createInstance(DesktopDefaultsSection)).element, this._register(instantiation.createInstance(DesktopCodecSection)).element, session, form, this.status, displays, processing); this.element.append(this.content);
		this._register(addDisposableListener(this.refresh, 'click', () => {
			void this.load();
		}));
		this._register(addDisposableListener(this.selector, 'change', () => {
			if (!this.busy) {
				void this.select();
			}
		}));
		this._register(addDisposableListener(form, 'submit', event => {
			event.preventDefault(); void this.save();
		}));
		this._register(this.editor.onDidChange(() => {
			this.status.textContent = '';
		}));
		this.setBusy(false);
		if (desktop.available) {
			void this.load();
		} else {
			this.refresh.disabled = true; this.empty.textContent = nls.localize('desktop.unavailable', 'The connection engine is unavailable. Restart VniLink to try again.'); this.codec.textContent = nls.localize('desktop.codecUnavailable', 'Video support could not be checked.');
		}
	}
	private disposed(): boolean {
		return this._store.isDisposed;
	}
	private setBusy(busy: boolean): void {
		this.busy = busy; this.content.setAttribute('aria-busy', String(busy)); this.refresh.disabled = busy; this.selector.disabled = busy || !this.settings?.sessions.length;
		this.editor.setEnabled(!busy && !!this.selected); this.apply.disabled = busy || !this.selected;
	}
	private render(settings: DesktopSettings, preferred?: string): void {
		this.settings = settings; this.sessionForm.hidden = !settings.sessions.length; this.selector.replaceChildren();
		for (const session of settings.sessions) {
			const option = document.createElement('option'); option.value = desktopTargetKey(session); option.textContent = session.role === 'Controller' ? nls.localize('desktop.viewing', 'Viewing {0} · #{1}', session.name, session.id) : nls.localize('desktop.sharing', 'Sharing with {0} · #{1}', session.name, session.id); this.selector.append(option);
		}
		this.selector.value = settings.sessions.some(session => desktopTargetKey(session) === preferred) && preferred ? preferred : settings.sessions[0] ? desktopTargetKey(settings.sessions[0]) : '';
		this.empty.textContent = settings.sessions.length ? '' : nls.localize('desktop.noSession', 'No active desktop session. Your saved defaults will be used when you connect.');
		const codecs = settings.codecs;
		const processors = [{ name: 'H.264', encode: codecs.encode_baseline || codecs.encode_high, decode: codecs.decode_baseline || codecs.decode_high, hardware: codecs.hardware }, ...(codecs.additional ?? []).map(codec => ({ ...codec, name: { H265: 'H.265', Vp8: 'VP8', Vp9: 'VP9', Av1: 'AV1' }[codec.codec] }))].filter(codec => codec.encode || codec.decode);
		this.codec.textContent = processors.length ? nls.localize('desktop.autoCodecSupport', 'Automatic selection uses a codec supported by both computers. Available here: {0}.', processors.map(codec => `${codec.name} (${codec.hardware ? nls.localize('desktop.hardware', 'hardware accelerated') : nls.localize('desktop.software', 'software processing')})`).join(', ')) : nls.localize('desktop.noCodec', 'Desktop streaming is not available in this build. You can still set the default quality without connecting to another computer.');
	}
	private async load(): Promise<void> {
		if (this.busy || this.disposed()) {
			return;
		}
		const preferred = this.selected ? desktopTargetKey(this.selected) : undefined; this.setBusy(true); this.status.textContent = '';
		try {
			const settings = await this.desktop.load(); if (this.disposed()) {
				return;
			} this.render(settings, preferred); await this.select();
		} catch {
			if (!this.disposed()) {
				this.selected = undefined; this.settings = undefined; this.selector.replaceChildren(); this.displays.replaceChildren(); this.editor.setValue(undefined); this.codec.textContent = nls.localize('desktop.codecUnavailable', 'Video support could not be checked.'); this.status.textContent = nls.localize('desktop.loadFailed', 'Could not load desktop sessions. Refresh to try again.');
			}
		} finally {
			if (!this.disposed()) {
				this.setBusy(false);
			}
		}
	}
	private async select(): Promise<void> {
		const revision = ++this.revision; this.selected = this.settings?.sessions.find(session => desktopTargetKey(session) === this.selector.value); this.editor.setValue(this.selected?.quality);
		this.displays.replaceChildren(); if (!this.selected) {
			this.displays.textContent = nls.localize('desktop.displaysPending', 'Displays appear when a desktop session connects.');
			this.setBusy(false); return;
		}
		const selected = this.selected; this.setBusy(true);
		try {
			const displays = await this.desktop.displays(selected); if (this.disposed() || revision !== this.revision) {
				return;
			}
			if (!displays.length) {
				const empty = document.createElement('p'); empty.className = 'settings-help'; empty.textContent = nls.localize('desktop.noDisplays', 'Display information has not been shared for this session.'); this.displays.append(empty);
			}
			for (const [index, display] of displays.entries()) {
				const row = document.createElement('div'); row.className = 'desktop-display-row'; const title = document.createElement('span'); title.textContent = nls.localize('desktop.display', 'Display {0}', String(index + 1)); const geometry = document.createElement('span'); geometry.textContent = nls.localize('desktop.displayGeometry', '{0} × {1} px · {2}%', String(display.width), String(display.height), String(display.scale_percent)); row.append(title, geometry); this.displays.append(row);
			}
		} catch {
			if (!this.disposed() && revision === this.revision) {
				this.displays.textContent = nls.localize('desktop.displayFailed', 'Could not load display information. Refresh to try again.');
			}
		} finally {
			if (!this.disposed() && revision === this.revision) {
				this.setBusy(false);
			}
		}
	}
	private async save(): Promise<void> {
		if (this.busy || !this.selected || this.disposed()) {
			return;
		}
		const quality = this.editor.value(); if (!quality) {
			this.status.textContent = nls.localize('desktop.invalid', 'Use 1–50,000 kbps, 1–60 fps, and dimensions between 1 and 7,680 px.'); return;
		}
		const target = this.selected; this.setBusy(true); this.status.textContent = '';
		try {
			const settings = await this.desktop.apply(target, quality); if (this.disposed()) {
				return;
			} this.render(settings, desktopTargetKey(target)); await this.select();
		} catch (error) {
			if (!this.disposed()) {
				this.selected = undefined; this.editor.setEnabled(false); this.status.textContent = error instanceof DesktopReadbackError ? nls.localize('desktop.readbackFailed', 'Limits were accepted, but the session could not be reloaded. Refresh to check its current state.') : nls.localize('desktop.saveFailed', 'Could not confirm the change. Refresh before editing this session again.');
			}
		} finally {
			if (!this.disposed()) {
				this.setBusy(false);
			}
		}
	}
	public override dispose(): void {
		super.dispose(); this.element.remove();
	}
}
