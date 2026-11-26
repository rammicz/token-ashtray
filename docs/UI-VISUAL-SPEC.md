# Native dashboard visual specification

The board uses a restrained dark dashboard. Official
provider marks remain unchanged; their sources and notices live in
`assets/providers/NOTICE.md`. Tray art and quota-chart internals have separate
ownership.

- **Hierarchy:** 18 pt application title, 16 pt provider name, 14 pt controls,
  12 pt metadata and supporting text. Provider plan and price occupy a quieter
  second header line. Quota values retain their existing size and meaning.
  Long names truncate within their allocated region and expose the full name
  on hover. Unavailable providers reserve a separate line for the reason.
- **Spacing:** 8 pt general rhythm, 12 pt card padding and inter-card gap,
  32 pt toolbar targets, 28 pt primary targets, and 8 pt corner radii. Narrow
  boards keep a single column; existing grid breakpoints remain.
- **Surfaces:** canvas `#101318`, cards `#191e26`, controls `#242c37`, subdued
  outlines `#364251`. Cards are flat, without gradients, shadows or bevels.
  Primary text `#eef2f7`, secondary text `#aebbc9`; secondary text must retain
  at least 4.5:1 contrast against card surfaces. Blue identifies interaction
  and selection. Status colors retain their existing consumption semantics.
- **Icons:** one locally painted vector family on a 16 pt grid, 1.5 pt strokes,
  centered within native egui controls. Refresh, settings, pin, primary,
  compact and drag share this vocabulary. Provider brand marks remain
  distinct. Icon-only controls expose names and hover descriptions.
- **States:** neutral controls use quiet outlines; hover uses a brighter
  surface and blue outline; keyboard focus keeps the native focus response.
  Selected primary uses a blue fill and outlined star. Disabled controls retain
  the native disabled treatment and the Wayland explanation. Menus retain
  native checkbox, focus, keyboard and accessibility semantics.

Review native before/after captures at measured 1× and 2× pixels per point,
including narrow and wide boards. Check long
names, dense lists, unavailable reasons, clipped edges, aligned controls,
contrast and consistent icon strokes. Exercise existing refresh, usage links,
ordering, primary, visibility, compact, pin and About actions. Synthetic IPC
fixtures and capture instrumentation stay outside the production source.
