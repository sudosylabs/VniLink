import * as nls from '../../../../nls.js';
import { INativeRuntimeService } from '../../../services/nativeRuntime/common/nativeRuntimeService.js';
import { IPermissionService } from '../../../services/permissions/common/permissionService.js';
import type { PowerPolicy } from '../../../../platform/engine/common/engine.js';
import { AccessToggle } from './accessToggle.js';
import { SettingsForm } from './settingsForm.js';
import { SettingsStateObserver } from './settingsStateObserver.js';
import './media/access.css';

export class PowerPage extends SettingsForm<Readonly<PowerPolicy>> {
	private readonly toggles = new Map<keyof PowerPolicy, AccessToggle>();
	private readonly incoming: HTMLInputElement;
	private readonly outgoing: HTMLInputElement;
	private readonly activity = document.createElement('p');

	constructor(@INativeRuntimeService private readonly runtime: INativeRuntimeService, @IPermissionService permissions: IPermissionService) {
		super('power', nls.localize('home.power', 'Power'), nls.localize('power.description', 'Keep this computer awake while remote sessions are in use.'), nls.localize('power.note', 'Applies to this computer, including sessions already running.'));
		const incoming = this.section(nls.localize('power.incoming', 'While sharing this computer'), nls.localize('power.incomingHelp', 'Sleep behavior while another device is connected. Set the idle timeout to 0 to keep desktop sessions open.'));
		this.toggle(incoming, 'incoming_system', nls.localize('power.system', 'Keep the computer awake'), nls.localize('power.systemHelp', 'Prevent system sleep while the session is being used.'));
		this.toggle(incoming, 'incoming_display', nls.localize('power.display', 'Keep the display awake'), nls.localize('power.displayHelp', 'Prevent the display from sleeping during the session.'));
		this.incoming = this.number(incoming, 'incoming_idle_secs', nls.localize('power.idle', 'End idle desktop sessions after'), nls.localize('power.minutes', 'min'), 0, 1440);
		const outgoing = this.section(nls.localize('power.outgoing', 'While controlling a remote computer'), nls.localize('power.outgoingHelp', 'Sleep behavior while you use another device. Set the idle timeout to 0 to keep desktop sessions open.'));
		this.toggle(outgoing, 'outgoing_system', nls.localize('power.system', 'Keep the computer awake'), nls.localize('power.systemHelp', 'Prevent system sleep while the session is being used.'));
		this.toggle(outgoing, 'outgoing_display', nls.localize('power.display', 'Keep the display awake'), nls.localize('power.displayHelp', 'Prevent the display from sleeping during the session.'));
		this.outgoing = this.number(outgoing, 'outgoing_idle_secs', nls.localize('power.idle', 'End idle desktop sessions after'), nls.localize('power.minutes', 'min'), 0, 1440);
		const ending = this.section(nls.localize('power.ending', 'After remote control ends'), nls.localize('power.endingHelp', 'Protect the computer when the last controlling session finishes.'));
		this.toggle(ending, 'lock_after_control', nls.localize('power.lock', 'Lock this computer'), nls.localize('power.lockHelp', 'Ask the operating system to lock when remote control ends.'));
		this.activity.className = 'settings-help';
		this.content.append(this.activity);
		this._register(new SettingsStateObserver(this.element, permissions.onDidFocusWindow, () => this.refresh()));
		void this.refresh();
	}

	private toggle(section: HTMLElement, key: keyof PowerPolicy, title: string, help: string): void {
		const toggle = this._register(new AccessToggle(title, help, () => {
			this.update();
		}));
		toggle.input.dataset.setting = key;
		toggle.setEnabled(true);
		this.toggles.set(key, toggle);
		section.append(toggle.element);
	}

	protected async read(): Promise<Readonly<PowerPolicy>> {
		const [policy, statuses] = await Promise.all([this.runtime.powerPolicy(), this.runtime.powerStatus()]);
		if (!this.disposed()) {
			const failures = statuses.filter(status => status.phase === 'Failed' || status.phase === 'Unavailable');
			this.activity.textContent = failures.length ? nls.localize('power.failed', 'Sleep prevention is unavailable: {0}', failures.map(status => status.error ?? status.phase).join(', ')) : '';
		}
		return policy;
	}

	protected write(policy: Readonly<PowerPolicy>): Promise<Readonly<PowerPolicy>> {
		return this.runtime.savePowerPolicy({ ...policy });
	}

	protected render(policy: Readonly<PowerPolicy>): void {
		for (const [key, toggle] of this.toggles) {
			toggle.setValue(policy[key] === true);
		}
		this.incoming.value = String(policy.incoming_idle_secs / 60);
		this.outgoing.value = String(policy.outgoing_idle_secs / 60);
	}

	protected draft(): PowerPolicy | undefined {
		const incoming = Number(this.incoming.value);
		const outgoing = Number(this.outgoing.value);
		if (!this.incoming.value || !this.outgoing.value || ![incoming, outgoing].every(value => Number.isFinite(value) && value >= 0 && value <= 1440 && Number.isInteger(value * 60))) {
			return undefined;
		}
		return { incoming_display: this.toggles.get('incoming_display')?.input.checked ?? false, incoming_system: this.toggles.get('incoming_system')?.input.checked ?? false, outgoing_display: this.toggles.get('outgoing_display')?.input.checked ?? false, outgoing_system: this.toggles.get('outgoing_system')?.input.checked ?? false, incoming_idle_secs: incoming * 60, outgoing_idle_secs: outgoing * 60, lock_after_control: this.toggles.get('lock_after_control')?.input.checked ?? false };
	}
}
