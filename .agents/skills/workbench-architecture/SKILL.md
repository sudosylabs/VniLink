---
name: workbench-architecture
description: "Write or review Electron/TypeScript workbench code (views, services, contributions, commands, IPC) in VS Code's style, using the devices feature as the in-repo reference."
---

# Workbench Architecture

## Two references

1. **VS Code** at the pinned commit in `build/vscode-source.json` (a VS Code checkout of the contributor's, with a worktree at that commit). Read the analogous upstream class before designing: `workbench/browser/workbench.ts`, `workbench/common/contributions.ts`, `platform/actions/common/actions.ts`, `base/browser/ui/list/listWidget.ts`.
2. **`src/workbench/contrib/devices`**, the VniLink feature written to this standard. New code should read like it. If it can't, say why.

## Shape of a feature

```
src/workbench/contrib/<feature>/
  <feature>.contribution.ts   registers pages, Action2 commands and workbench contributions
  <view>.ts                   views and their child widgets
  media/<feature>.css         its styles
src/workbench/services/<area>/
  <area>Service.ts            interface + createDecorator
  <area>ServiceImpl.ts        implementation, ending with registerSingleton(...)
```

Every new contribution or service implementation is imported from `src/workbench/workbench.desktop.main.ts`. Features never import each other's views. Home finds pages through `HomePagesRegistry`, and toolbars find actions through menus.

## Views

```ts
export class DeviceDetails extends Disposable {
	public readonly element: HTMLElement;

	constructor(
		id: string,                                            // plain arguments first
		@IDeviceService private readonly devices: IDeviceService, // then injected services
		@IInstantiationService instantiationService: IInstantiationService,
	) {
		super();
		const panel = h('aside.device-panel', [h('h2@title'), h('p.device-panel-state@state', { role: 'status' })]);
		this.element = panel.root;
		this.screen = this._register(instantiationService.createInstance(DeviceScreenSnapshot));
		this.connect = this._register(new Button(this.element, { ...defaultButtonStyles, title: false }));
		this._register(this.connect.onDidClick(() => this.run(id => this.devices.connect(id))));
	}
}
```

- **DOM:** build it with `h`, `$` and `append` from `base/browser/dom.ts`. `document.createElement` and `element.click()` fail lint (`vnilink/workbench-dom`). Use `triggerUpload` and `triggerDownload` for file pickers and downloads.
- **Widgets:** use VS Code's widgets for controls:
  - `List` (`deviceList.ts`)
  - `Button` and `InputBox` (`deviceDetails.ts`, `devicePairing.ts`)
  - `ActionBar` (`viewActionsToolbar.ts`)

  Pass styles from `platform/theme/browser/defaultStyles.ts` and adjust appearance in the feature CSS. Lists use a CSS style controller (see `deviceList.ts`), so no inline `<style>` element breaks the CSP.
- **Children:** create them with `instantiationService.createInstance`, never `new` with services passed by hand.
- **Ownership:** register every listener, child and timer (`IntervalTimer`, `RunOnceScheduler`). After an `await`, re-read disposal through a method (`isDisposed()`), and fence late results by id or revision (`DeviceDetails.run`).
- **Constructors:** split long ones into `registerListeners()` and `render()`/`update()`.

## Services and processes

- **Renderer services** inject other services. They never touch Electron or the preload. Engine work goes through `IEngineService`, and window and OS work through `INativeHostService`.
- **Main-process services** live in `platform/<area>/electron-main`. Each is exposed through a channel facade registered in `VniLinkApplication.initChannels` (`src/code/electron-main/app.ts`). The facade validates arguments with the `common` guards and takes window identity from the IPC context (`windowIdFromContext`). See `engine-bridge` for engine commands.
- **Remote stubs:** a renderer service that is only a main-process proxy uses `registerMainProcessRemoteService` (`platform/recording/electron-browser/recordingService.ts`).

## Commands, menus and startup

- **Toolbar buttons** are `registerAction2` classes with `menu: { id: ViewActionsMenu, when }`, gated by context keys from `src/workbench/contextkeys.ts` (`actions/applicationActions.ts`).
- **New context keys** are defined there and bound by a contribution (`contextkeysHandler.ts`).
- **Startup work** is `registerWorkbenchContribution2(id, ctor, WorkbenchPhase.X)`. Choose the latest phase that still works.

## Classes and functions

Stateful owners are classes: views, services, contributions, process owners. Pure transforms and guards are functions. Build and packaging code is functions too; see `build-test-infrastructure`.

## Copying from VS Code

Never hand-copy. Use the importer:

| Command | What it does |
|---|---|
| `node build/run.ts vscode import <checkout> src/vs/...` | Copies the file and its closure |
| `adapt [--upstream-style] <paths>` | Records a reviewed edit |
| `reset <checkout> <paths>` | Restores upstream text |
| `sync` | Re-derives entry points after imports change |

Rules:
- An adapted file names its adaptation in its header.
- `--upstream-style` keeps VS Code's formatting and lint baseline for infrastructure.
- `npm run check:upstream` must pass.
- Copy only what a consumer in the same change uses.

## Review bar

Before finishing, answer these against the devices reference:
- Who owns each piece of state?
- Which service boundary does the code cross?
- Where is it registered?
- How is it disposed?
- Which widget renders each control?
- Which upstream class did you compare with?

Existing code outside the area you were asked to change is not precedent. Report its debt instead of copying it.
