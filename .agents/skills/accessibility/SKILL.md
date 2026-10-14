---
name: accessibility
description: "Implement accessible VniLink interactions, keyboard/focus behavior, screen-reader semantics and shared help/announcement infrastructure for new or changed UI."
---

# Accessibility

Every new or changed interactive surface must be operable and understandable with keyboard and assistive technology. Apply this skill even when accessibility is not named in the request.

## Baseline behavior

- Use native semantics when they express the control accurately; custom widgets expose role and selected/checked/expanded state. Associate visible labels/help with their controls using label elements or `aria-labelledby`/`aria-describedby`.
- Icon-only actions have localized action labels. Decorative icons are hidden from the accessibility tree. Keep logical tab order and appropriate arrow-key navigation for tablists, toolbars, lists and trees; pointer-only actions need keyboard equivalents.
- Focus remains visible. Modals trap focus; dismissible overlays support Escape and restore focus. Page/tab changes, failed operations and disposal do not leave focus on removed elements.
- Announce important dynamic results once. Prefer polite status for ordinary progress/completion; reserve assertive alerts for urgent interruptions. Avoid duplicating visible status with multiple live regions/signals.
- Disabled/unavailable states explain the actual reason and preserve access to that explanation. Loading, validation and errors remain accessible. Respect reduced motion and forced colors (`ux-theming`).

## Shared help and rich content

Use VS Code's accessibility architecture as the target for shared help, accessible views, verbosity and signals. Inspect the corresponding upstream modules before implementing it. A feature with substantial shortcuts or non-trivial visual content needs appropriate discoverability and an equivalent accessible representation; a basic labeled setting does not require a new read-only editor just to complete a checklist.

When a feature uses shared accessibility help, document its actual commands, resolve shortcuts dynamically and update help when interactions change. Use a verbosity setting for repeated discovery hints where the shared infrastructure supports it. Reuse existing signal vocabulary; new sounds/announcements need a product purpose rather than one per action.

Upstream entry points relative to the VS Code checkout:

- `src/vs/platform/accessibility/browser/accessibleView.ts` and `accessibleViewRegistry.ts`.
- `src/vs/workbench/contrib/accessibility/browser/accessibilityConfiguration.ts`.
- `src/vs/platform/accessibilitySignal/browser/accessibilitySignalService.ts`.
- `src/vs/base/browser/ui/aria/aria.ts`.

These foundations are not all integrated in VniLink. Explicitly include required foundation work in the feature's scope or report its dependency; do not invent per-page substitutes or claim the imports exist. Copied infrastructure follows `workbench-architecture` provenance.

## Verification

Exercise keyboard navigation, focus restoration, labels and dynamic announcements in representative states. Browser tests can verify semantics/behavior, but they do not prove what VoiceOver/NVDA or Linux assistive technology announces. For changes affecting screen-reader behavior, obtain native evidence where available and name manual/unverified checks. Validation follows `validate-ui-scenario` and `cross-platform-validation`.
