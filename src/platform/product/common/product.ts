import metadata from '../../../../product.json' with { type: 'json' };

/** Runtime identity is derived from the same metadata as packaged application resources. */
export const product = Object.freeze({
	name: metadata.nameLong,
	appId: metadata.appId,
});
