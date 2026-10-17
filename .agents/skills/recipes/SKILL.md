---
name: recipes
description: "Step lists for common VniLink changes: a Home page, a workbench service, a main-process capability, a toolbar action, a startup contribution, copying VS Code code. Use when adding one of these."
---

# Recipes

Each recipe names the files to touch and the existing file to imitate. `workbench-architecture` explains the rules behind them.

## Add a Home page

1. Write the page class in `src/workbench/contrib/<feature>/<name>Page.ts`, implementing `IHomePage` from `contrib/home/homePages.ts`. Imitate `contrib/devices/devicesPage.ts`: inject services, start the element with `createPageHeading`, create children with `createInstance`.
2. Register it in `<feature>.contribution.ts` with `Registry.as<IHomePagesRegistry>(Extensions.HomePages).registerPage({ id, group, order, title, icon, ctorDescriptor: new SyncDescriptor(Page) })`. Imitate `contrib/devices/devices.contribution.ts`.
3. Import the contribution from `src/workbench/workbench.desktop.main.ts`.
4. Add a scenario in `test/browser/` that builds the page through an `InstantiationService` with fake services. Imitate `test/browser/devices.ts`.

## Add a workbench service

1. Contract: `src/workbench/services/<area>/<area>Service.ts` with `export const IAreaService = createDecorator<IAreaService>('areaService')` and the interface.
2. Implementation: `<area>ServiceImpl.ts`, constructor parameters `@IEngineService ...`, last line `registerSingleton(IAreaService, AreaService, InstantiationType.Delayed)`. Imitate `services/devices/deviceServiceImpl.ts`.
3. Import the implementation from `workbench.desktop.main.ts`.
4. Contract tests in `test/tooling/<area>.test.mjs` with `fakeEngine` from `test/tooling/fixtures.mjs`.

## Add a main-process capability for windows

1. Contract and guard in `src/platform/<area>/common/<area>.ts`, with a `<AREA>_CHANNEL_NAME` constant.
2. Main service in `platform/<area>/electron-main/<area>MainService.ts`, registered in `VniLinkApplication.createServices`.
3. Channel facade in `platform/<area>/electron-main/<area>Channel.ts`. It validates every argument and reads the window from `windowIdFromContext(context)`. Imitate `platform/permissions/electron-main/permissionsChannel.ts`. Register it in `initChannels`.
4. Renderer side: `registerMainProcessRemoteService` for plain proxies, or a small service over `IMainProcessService.getChannel` when results need validation (`platform/configuration/electron-browser/configurationSnapshotService.ts`).
5. Channel-level tests in `test/tooling/applicationIpc.test.mjs`, including refused input.

## Add a toolbar action

1. Define or reuse a context key in `src/workbench/contextkeys.ts`, bound in `contextkeysHandler.ts` or the owning contribution.
2. `registerAction2(class extends Action2 { ... menu: { id: ViewActionsMenu, group: 'navigation', when } ... run(accessor) { return accessor.get(IService).doIt(); } })` in the feature's contribution or `src/workbench/actions/`. Imitate `actions/applicationActions.ts`. Give `icon: ThemeIcon.fromId(Icons.x)` for product icons.
3. Test the menu through real `MenuService`/`ContextKeyService`, as `test/browser/home.ts` does for restart.

## Add startup work

`registerWorkbenchContribution2(ID, Contribution, WorkbenchPhase.BlockRestore | AfterRestored | Eventually)`. Imitate `contrib/home/home.contribution.ts`. Prefer the latest phase that still works.

## Copy code from VS Code

`node build/vscodeImport.mjs import <checkout at the pinned commit> src/vs/<path>` copies it with its closure. Edit only what the product requires, add an `Adapted for VniLink:` header line, then run `node build/vscodeImport.mjs adapt [--upstream-style] <paths>` and `npm run check:upstream`.
