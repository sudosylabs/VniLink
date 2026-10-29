---
name: design-philosophy
description: "Design or review VniLink surfaces for integrated window/content continuity, clear hierarchy, meaningful controls and truthful remote-access state rather than generic page templates."
---

# VniLink Design Philosophy

## Product direction

The interface should feel like a deliberately composed desktop application. VS Code is the reference for workbench integration and selected interactions, not a requirement to reproduce an editor. RustDesk and user-supplied screenshots clarify remote-access behavior and visual intent; read them rather than inferring from the current conversation topic.

- Window chrome, titlebar tabs and their content form a continuous composition. The active tab joins its content surface through background, border and corner treatment; it should not look like a floating card detached from the page.
- Home is a permanent, unclosable tab. Its sidebar belongs to its content. Other tab content can be a desktop, terminal or another remote capability; active-tab actions stay in a reserved far-right toolbar before native right-side controls. Tabs scroll before that area. Empty tab-strip space remains draggable while controls remain interactive.
- Favor clear typography and purposeful grouping, comfortable spacing, centered bounded content on wide windows and bottom breathing room on scrollable pages. A repeated card grid, oversized heading or extra divider needs a role; decorative density does not improve hierarchy.
- Theme previews are stylized illustrations that resemble the app's visual geometry, not miniature working pages. Use the user's reference to choose detail and proportion.
- Product icons use the shared Phosphor helper; consistent outlined/filled states communicate selection. Native materials and readable fallbacks follow `ux-theming`.

## Behavior and language

Global settings establish sensible defaults for future connections. Per-device/session overrides belong with that context when they have a meaningful purpose. Labels distinguish local permissions, incoming authorization and outgoing preferences.

Controls show real saved, engine or OS state. Unsupported functionality requires an accurate capability explanation; fixture data and working-looking placeholders are not production implementations. A known RustDesk capability on the same OS is reason to investigate our backend/support detection, not to label the OS incapable.

Validation appears only in its relevant mode and preserves drafts on failure. Save/apply is enabled when valid changes differ from the saved baseline. Restart actions appear when actually required. Explain consequences (such as relay provider/rate limits), with authoritative links where useful. Avoid routine reassurance like "Saved on this device" and implementation jargon that does not help the user decide.

## Method and proof

Describe the intended feeling, diagnose the hierarchy/continuity/interaction issue, then choose a concrete move grounded in the reference and shared components. Existing UI is not the design ceiling. Evaluate loading, empty, error and unavailable states as part of the design.

Use `ux-css-layout`, `ux-theming` and `accessibility` for implementation; validate relevant visual and behavioral states with `smoke-tests`, including narrow/wide and supported platform presentations. Passing a code check does not establish design quality.
