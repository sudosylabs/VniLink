import * as nls from '../../../../nls.js';
import { Disposable } from '../../../../base/common/lifecycle.js';
import { Emitter } from '../../../../base/common/event.js';
import { addDisposableListener } from '../../../../base/browser/domEvent.js';
import { renderIcon } from '../../../../base/browser/ui/icon/icon.js';
import { Icons } from '../../../../base/common/icons.js';
import type { DeviceSummary } from '../../../services/devices/common/deviceService.js';
import { devicePlatform, devicePresence } from './deviceLabels.js';

/** Stable row templates keep keyboard focus during lifecycle updates. */
class DeviceRow extends Disposable {
	public readonly element = document.createElement('li');
	public readonly button = document.createElement('button');
	private readonly name = document.createElement('span');
	private readonly emblem = document.createElement('span');
	private os: DeviceSummary['os'] | undefined;
	private readonly presence = document.createElement('span');

	constructor(public readonly id: string, panelId: string, select: () => void) {
		super();
		this.button.type = 'button';
		this.button.className = 'device-row';
		this.button.dataset.deviceId = id;
		this.button.setAttribute('aria-controls', panelId);
		const caption = document.createElement('span');
		caption.className = 'device-caption';
		this.name.className = 'device-name';
		caption.append(this.name, this.presence);
		this.presence.className = 'device-presence';
		this.emblem.className = 'device-emblem';
		this.button.append(this.emblem, caption, renderIcon(Icons.chevronRight));
		this.element.append(this.button);
		this._register(addDisposableListener(this.button, 'click', select));
	}

	public update(device: DeviceSummary, selected: boolean): void {
		this.name.textContent = device.name;
		this.name.title = device.name;
		if (this.os !== device.os) {
			this.os = device.os;
			const icon = device.os === 'macos' ? Icons.apple : device.os === 'windows' ? Icons.windows : device.os === 'linux' ? Icons.linux : Icons.remoteDesktop;
			this.emblem.replaceChildren(renderIcon(icon, { variant: 'filled', label: devicePlatform(device) }));
		}
		this.presence.textContent = devicePresence(device);
		this.presence.dataset.presence = device.presence;
		this.button.setAttribute('aria-pressed', String(selected));
		this.button.setAttribute('aria-expanded', String(selected));
	}

	public override dispose(): void {
		super.dispose();
		this.element.remove();
	}
}

export class DeviceList extends Disposable {
	public readonly element = document.createElement('ul');
	private readonly rows = new Map<string, DeviceRow>();
	private readonly selected = this._register(new Emitter<string>());
	public readonly onDidSelect = this.selected.event;

	constructor(private readonly panelId: string) {
		super();
		this.element.className = 'device-list';
		this.element.setAttribute('aria-label', nls.localize('devices.list', 'Remembered computers'));
		this._register(addDisposableListener(this.element, 'keydown', event => {
			const key = (event as KeyboardEvent).key;
			if (!['ArrowDown', 'ArrowUp', 'Home', 'End'].includes(key)) {
				return;
			}
			const buttons = [...this.element.querySelectorAll<HTMLButtonElement>('button')];
			const current = buttons.findIndex(button => button === document.activeElement);
			if (current < 0) {
				return;
			}
			event.preventDefault();
			const index = key === 'Home' ? 0 : key === 'End' ? buttons.length - 1 : Math.max(0, Math.min(buttons.length - 1, current + (key === 'ArrowDown' ? 1 : -1)));
			buttons[index]?.focus();
		}));
	}

	public update(devices: readonly DeviceSummary[], selectedId: string | undefined): void {
		const ids = new Set(devices.map(device => device.id));
		for (const [id, row] of this.rows) {
			if (!ids.has(id)) {
				const focused = row.element.contains(document.activeElement);
				row.dispose();
				this.rows.delete(id);
				if (focused) {
					this.rows.values().next().value?.button.focus();
				}
			}
		}
		for (const [index, device] of devices.entries()) {
			let row = this.rows.get(device.id);
			if (!row) {
				row = new DeviceRow(device.id, this.panelId, () => {
					this.selected.fire(device.id);
				});
				this.rows.set(device.id, row);
			}
			row.update(device, selectedId === device.id);
			if (this.element.children[index] !== row.element) {
				this.element.insertBefore(row.element, this.element.children[index] ?? null);
			}
		}
	}

	public focus(id?: string): void {
		(this.rows.get(id ?? '') ?? this.rows.values().next().value)?.button.focus();
	}

	public override dispose(): void {
		for (const row of this.rows.values()) {
			row.dispose();
		}
		this.rows.clear();
		super.dispose();
	}
}
