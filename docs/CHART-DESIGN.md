# Quota chart design

The native board uses aligned horizontal quota meters.
Selection followed an actual native comparison against refined rings using the
same synthetic snapshot, fixed fixture clock, fonts, mode and 360/1040-point
viewports.

## Why aligned meters

Both prototypes wrapped the complete model/window name and displayed reset and
status text without hover. The refined-ring prototype used one ring per row,
with its full label and reset beside it, so it was not handicapped by the old
three-column layout.

| Criterion | Refined rings | Aligned meters |
| --- | --- | --- |
| Complete long names | Readable, wrapping beside each ring | Readable, wrapping above the aligned track |
| Comparing six independent windows | Requires comparing separate sweeps | Common zero baseline and aligned values |
| Positive fraction below 1% | Rounded arc caps visually amplify a tiny fraction | Fill width retains the reported proportion; `<1%` is explicit |
| Value and meaning | Meaning fits inside a small ring | One selected value centered on the track; explicit mode in tooltip/accessibility |
| Vertical space | About 120 points less for the six-window fixture | Compact rows retain comparison clarity and complete names |

Aligned meters won on value comparison and proportional encoding. The
owner-approved compact-row refinement removed redundant quota captions and mode rows.
A later refinement places one selected value over a continuous track and
moves a duration-only reset countdown to the title's right. Complete labels
wrap when needed; scrolling is preferable to ellipses or hover-only names.

## Applied rules

- Each independent quota window has its complete 14-point wrapping name and,
  when known, a right-aligned 12-point duration-only countdown on that title
  row (for example `6d 23h`). The title reserves the countdown's width and
  wraps without clipping. Extremely narrow budgets can move the countdown
  onto another line. A missing reset leaves no countdown or reserved slot.
- Every uninterrupted 20-point track represents 0–100%. Its one 12-point
  selected percentage is centered horizontally and vertically on the bar.
  Fixed endpoint/midpoint labels and ticks and the separate scale/value row
  are absent. Fill begins at zero and remains proportional, with no
  minimum-length inflation; a positive share that rounds to 0 shows `<1%`,
  and zero has no fill. No history, trend or usage count is inferred.
- Every percentage label (meter, tooltip, accessibility text and tray
  tooltip, for every provider) shows a whole percent. The used share rounds
  half up, so 50.5% used shows `51%`. Remaining shows the complement of that
  rounded value, so the same meter shows `49%` Remaining. The two modes add
  up to 100 except where `<1%` is shown: a positive share that rounds to 0
  shows `<1%`, so 99.6% used shows `100%` Used and `<1%` Remaining. The fill
  width keeps the exact value.
- White value glyphs have a thin black glyph outline. There is no opaque badge
  or cutout interrupting the track. The foreground or its outline stays
  contrasted against filled and unfilled regions in either theme, including
  a fill boundary crossing the number. Outline glyph passes are decoration around
  the one foreground label, not another value or accessibility node.
- Ordinary quota captions and standalone Used/Remaining rows stay absent.
  The display preference, tooltip and accessible value/name retain explicit
  Used or Remaining meaning. Tooltip/accessibility retain `Resets in …` or
  `Reset unavailable`, even though the visible countdown uses duration only.
- The existing consumption thresholds remain: remaining <=25% warns, <=10% is
  critical. Normal windows retain the existing accent palette. Changing the
  display setting changes the number and extent, never consumption severity.
  Visible High usage, Nearly exhausted and Allowance exhausted cues distinguish
  those states without color. Stale cached quota is labelled and muted.
- Row allocation measures the title/countdown, track and any visible status
  message. Reset-only and numeric-scale rows reserve no space. Status messages
  such as Allowance exhausted or Stale cached quota remain below the track;
  freshness remains a wrapping Last updated line when an age is available.
  Equal-width quota cells share their row's measured header height so their
  tracks align, and the narrow-width wrapping and equal provider cards remain.
- Unavailable providers retain their error/reconnect reason; absent readings
  do not become 0%.
- Product attribution is a separate Shared quota used section, explicitly
  describing one shared allowance. Full product names wrap, every value is Used,
  and products have no independent reset or quota. Attribution stays Used even
  when allowance windows show Remaining. No remainder or total is invented.
- Real egui interaction responses retain usage links, keyboard focus, accessible
  full wording and numeric range 0–100. The board surfaces, provider marks,
  interface icons, ordering and window behavior remain intact.

## References and adaptation

[ONS chart elements](https://service-manual.ons.gov.uk/data-visualisation/build-specifications/chart-elements)
recommends direct labels where practical, consistent chart furniture and limited
decoration. The meters therefore use plain tracks and direct values, without
nested decorative cards or a color legend.

[ONS axes and gridlines](https://service-manual.ons.gov.uk/data-visualisation/guidance/axes-and-gridlines)
requires a zero baseline for filled bars, favors consistent linear scales and
reduces gridline count for smaller views. The compact native meters adapt that
guidance with a common implicit 0–100 range and one direct selected value;
the owner-approved compact treatment removes visible axis labels.

[Carbon's overview](https://www.carbondesignsystem.com/building-blocks/data-visualization/overview)
offers both gauge and meter treatments. Its [gauge guidance](https://charts.carbondesignsystem.com/gauge)
notes that a single-value gauge can oversimplify complex data; its
[meter guidance](https://charts.carbondesignsystem.com/meter) describes a simple
horizontal range display for dashboards. These informed the native comparison,
not a requirement to add Carbon's JavaScript stack.

## Regression evidence

The narrow-label guard inspects actual painted glyph characters and glyph-mesh
vertices inside the viewport clip, including full product/reset text. It fails
on the former renderer's ellipsis despite its already complete accessible name.
The extent/overlay guard checks one foreground value on one uninterrupted
track, no ticks or numeric row, a zero baseline, exact selected fraction, zero/full and
positive-fraction cases while retaining value/range/meaning, consumption color
and stale semantics. Model/window identity and shared-attribution tests remain.
Native evidence and independent QA cover readable scrolling, grayscale cues,
focus and pointer/keyboard link activation. No cosmetic coordinate snapshots
replace those behavioral checks.

The countdown fixture freezes wall time at Unix 1800000000 for the exact `6d 23h`
countdown. It compares the actual before/after native ELF in Light and Dark,
the board, at 360 and 1000 logical points.
[Native fixture instructions](../tests/responsive_layout/README.md) include
fully used Claude, positive fractions, a split boundary under the number,
missing reset, stale freshness and distinct shared attribution. The clock
interposer affects only the test app's wall clock; monotonic/window timers
and the renderer are unchanged.
