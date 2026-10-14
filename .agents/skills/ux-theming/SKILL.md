---
name: ux-theming
description: "Implement semantic theme colors, native vibrancy/Mica, opaque fallbacks and accessible interaction states across light, dark and forced-colors presentations."
---

# Workbench Theming

## Target and migration

Semantic color roles have shared owners and consistent interaction states. Reuse or introduce the appropriate token rather than repeating literal colors in product components. Palette definitions, intentional preview artwork and controlled fixtures may contain literal source colors at their owning layer. Preserve pinned upstream styles as defined by `workbench-architecture`.

The current theme service is titlebar-focused and workbench CSS also defines variables; a complete theme registry/high-contrast theme must not be assumed to exist. Changes requiring richer theme infrastructure include an explicit scoped foundation/migration, rather than weakening the target or claiming support from a hardcoded override.

## Native materials

macOS vibrancy and supported Windows Mica are native window effects, not CSS effects independently attachable to any div. Coordinate the main-process material/theme with renderer transparency: selected areas reveal the native background, other surfaces remain opaque. Use real availability and OS version/session constraints. Unsupported/disabled material uses a coherent opaque theme.

Check light and dark foreground readability, focused/unfocused window states and theme changes at runtime. Avoid double tinting or an unexplained washed-out light surface. A macOS inspection does not establish Windows Mica behavior; see `cross-platform-validation`.

## Interaction and accessibility

Hover, active, selected, disabled and keyboard focus have consistent semantic styling. Keep a visible focus indicator and sufficient contrast. Do not encode meaningful state by color alone; use labels/icons/shapes and appropriate ARIA state. Honor forced-colors and reduced-motion preferences. OS forced-colors testing and a product high-contrast theme are distinct evidence.

Use `accessibility` and `validate-ui-scenario` for relevant inspections. Report missing theme foundations or untested native materials accurately.
