---
name: ux-css-layout
description: "Implement workbench layout, tab/content geometry, scrolling and responsive spacing with shared component styles and container-aware sizing."
---

# Workbench Layout

`design-philosophy` owns product visual intent. Styles belong beside their owning component (including its `media/` directory); shared geometry/tokens have one owning layer. Class names identify roles, not colors or arbitrary appearance.

Reuse established spacing/radius/icon steps when they fit the design. If shared ramps are incomplete, define deliberate reusable values at their owner rather than copying magic numbers between pages. New values are justified by role/reference, not by an obligation to leave poor existing geometry unchanged.

Use layout owners, grids or SplitView as appropriate to the subsystem studied upstream. Parts own resizing/sashes; their content scrolls inside the assigned region. Handle min sizes, flex shrink and overflow explicitly. Content is centered at wide widths, adapts at narrow widths and retains bottom padding when scrolled to the end.

Titlebar height and tab height are independent. Preserve active-tab/content continuity, native-control reservations and the far-right action area. Empty strip space is draggable; tabs, close buttons and actions are not. Test fractional zoom and OS-specific control geometry.

Prefer container-aware sizing for regions that can resize independently. Use window sizing only for actual window-level behavior. Single-line contractual labels truncate with a discoverable full value; text intended to explain behavior wraps. Avoid silent clipping and accidental horizontal page scrolling.

Resolve specificity through component scope rather than new `!important` overrides. Preserve intentional upstream rules in fidelity-controlled copies. Verify covering CSS lint and inspect relevant narrow/wide, zoom and scrolling states through `smoke-tests`; `unit-tests` owns test selection.
