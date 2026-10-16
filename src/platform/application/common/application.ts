export interface RestartState { readonly revision: number; readonly required: boolean; }

export function isRestartState(value: unknown): value is RestartState {
	return typeof value === 'object' && value !== null && 'revision' in value && Number.isSafeInteger(value.revision) && Number(value.revision) >= 0 && 'required' in value && typeof value.required === 'boolean' && Object.keys(value).length === 2;
}
