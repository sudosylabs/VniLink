import type { Event } from '../../../base/common/event.js';
import { createDecorator } from '../../../platform/instantiation/common/instantiation.js';
import type { PairingFile } from './deviceService.js';

/**
 * Where pairing stands. Creating an invitation waits for the other computer
 * and may turn into approving its request; opening one asks for the other
 * computer's digits and then shows this computer's digits to read out.
 */
export type PairingStep =
	| { readonly kind: 'idle' }
	/** An invitation is live; its file can be saved and its code read out. */
	| { readonly kind: 'invitation'; readonly file: PairingFile }
	/** Another computer opened the invitation; its digits approve it. */
	| { readonly kind: 'approval'; readonly endpointId: string }
	/** An invitation was opened; the creator's digits pair with it. */
	| { readonly kind: 'opened'; readonly fileName: string }
	/** Paired from an invitation; the creator approves with these digits. */
	| { readonly kind: 'reverseCode'; readonly code: string; readonly endpointId: string }
	/** Both computers trust each other. */
	| { readonly kind: 'paired' }
	/** The invitation expired or its request was rejected; start again. */
	| { readonly kind: 'ended' };

export interface PairingState {
	readonly step: PairingStep;
	/** A command is running; controls stay disabled. */
	readonly busy: boolean;
	/** The latest outcome or error to read out, or empty. */
	readonly message: string;
}

export const IDevicePairingService = createDecorator<IDevicePairingService>('devicePairingService');

/**
 * Pairing owned by the window, so it survives the popover that shows it.
 * Policy stays in the engine; this carries files and digits.
 */
export interface IDevicePairingService {
	readonly _serviceBrand: undefined;
	readonly state: PairingState;
	readonly onDidChange: Event<PairingState>;
	/** Creates an invitation for this computer and waits for the other computer. */
	createInvitation(): Promise<void>;
	/** Opens an invitation another computer created. */
	openInvitation(file: File): Promise<void>;
	/** Approves a request or pairs with an opened invitation, depending on the step. */
	submitCode(digits: string): Promise<void>;
	/** Refuses the incoming request. */
	reject(): Promise<void>;
	/** Forgets a finished or abandoned flow. A live invitation keeps waiting until it expires. */
	reset(): void;
}
