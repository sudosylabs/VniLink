import { app } from 'electron';
import { isWindows } from '../../base/common/platform.js';
import { product } from '../../platform/product/common/product.js';
import { VniLinkApplication } from './app.js';

/** Establishes process identity and explicit smoke-test isolation before services read paths. */
export class VniLinkMain {
	public async main(): Promise<void> {
		app.setName(product.name);
		if (isWindows) {
			app.setAppUserModelId(product.appId);
		}
		const temporary = process.argv.includes('--smoke-test');
		if (temporary) {
			const profile = process.env.VNILINK_TEST_PROFILE;
			if (!profile) {
				throw new Error('--smoke-test requires an isolated VNILINK_TEST_PROFILE');
			}
			app.setPath('userData', profile);
		}
		await new VniLinkApplication({ temporaryEngine: temporary }).start();
	}
}
