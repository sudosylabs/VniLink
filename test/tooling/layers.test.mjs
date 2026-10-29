import { RuleTester } from 'eslint';
import tseslint from 'typescript-eslint';
import { layersRule } from '../../build/lint/layers.ts';

const tester = new RuleTester({ languageOptions: { parser: tseslint.parser } });

tester.run('vnilink/layers', layersRule, {
	valid: [
		{ filename: 'src/base/browser/ui/icon/iconAssets.ts', code: "import svg from '@phosphor-icons/core/assets/regular/house.svg';" },
		{ filename: 'src/workbench/workbench.ts', code: "import type { INativeWindowConfiguration } from '../../platform/window/common/windowConfiguration';" },
		{ filename: 'src/code/electron-main/app.ts', code: "import { app } from 'electron';" },
		{ filename: 'src/base/browser/ui/button.ts', code: "export const button = document.createElement('button');" },
	],
	invalid: [
		{ filename: 'src/workbench/workbench.ts', code: "import svg from '@phosphor-icons/core/assets/regular/house.svg';", errors: [{ messageId: 'boundary' }] },
		{ filename: 'src/platform/engine/node/service.ts', code: "import '../electron-main/service.js';", errors: [{ messageId: 'boundary' }] },
		{ filename: 'src/platform/ipc/common/types.ts', code: "type X = import('../../../preload.js').X;", errors: [{ messageId: 'boundary' }] },
		{ filename: 'src/platform/engine/electron-utility/host.ts', code: "import '../electron-main/service.js';", errors: [{ messageId: 'boundary' }] },
		{ filename: 'src/platform/ipc/common/types.ts', code: "import('../../../preload.js');", errors: [{ messageId: 'boundary' }] },
		{ filename: 'src/workbench/workbench.ts', code: "const fs = require('fs');", errors: [{ messageId: 'boundary' }] },
		{ filename: 'src/platform/ipc/common/types.ts', code: "import '../../../renderer.js';", errors: [{ messageId: 'boundary' }] },
		{ filename: 'src/workbench/workbench.ts', code: "import fs from 'node:fs';", errors: [{ messageId: 'boundary' }] },
		{ filename: 'src/workbench/contrib/devices/devicesPage.ts', code: "import { ipcRenderer } from 'electron';", errors: [{ messageId: 'boundary' }] },
		{ filename: 'src/workbench/services/devices/deviceServiceImpl.ts', code: "import { EngineChannel } from '../../../platform/engine/electron-main/engineChannel.js';", errors: [{ messageId: 'boundary' }] },
		{ filename: 'src/workbench/workbench.ts', code: "export { app } from 'electron';", errors: [{ messageId: 'boundary' }] },
		{ filename: 'src/workbench/workbench.ts', code: "const x = import('../../platform/ipc/electron-main/windowContext');", errors: [{ messageId: 'boundary' }] },
		{ filename: 'src/platform/ipc/common/types.ts', code: "import { createSidebar } from '../../../workbench/browser/parts/sidebar/sidebar';", errors: [{ messageId: 'boundary' }] },
		{ filename: 'src/base/browser/ui/button.ts', code: "import { createSidebar } from '../../../workbench/browser/parts/sidebar/sidebar';", errors: [{ messageId: 'boundary' }] },
		{ filename: 'src/platform/ipc/electron-main/windowContext.ts', code: "import { VniLinkApplication } from '../../../code/electron-main/app';", errors: [{ messageId: 'boundary' }] },
	],
});
