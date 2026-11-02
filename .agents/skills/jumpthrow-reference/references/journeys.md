# jumpthrow reference journeys

All run in the built-in browser with [`scripts/sheet.js`](../scripts/sheet.js) pasted into `javascript_tool` after
`navigate` and a 1.5 s wait. Evidence (screenshots, `info.json`) goes in a scratch folder outside the repo. A page
without a `<video>` or a site that refuses the browser is a gap, not a pass.

| # | Journey | Entry point | Success |
|---|---|---|---|
| 1 | Read a nade page | `navigate` `/nades/<uuid>`, paste `sheet.js`, `jt.info()` | video URL, duration, 1920×1080, map, type, airtime, buttons, `setpos`/`setang`, two still URLs; save as `info.json` |
| 2 | Whole-video timeline | `await jt.sheet(jt.range(0, jt.info().duration, 0.5), 8)`, then `screenshot` | one labelled sheet showing spot, aim, zoom, lineup, throw, chase, aftermath (the table in [findings](findings.md)) |
| 3 | Phase boundaries | `jt.sheet([...each start in template.json and 0.1 s before it], 4)` | each pair straddles its cut; `python3 scripts/template.py --at <t>` names the phase seen; update `template.json`, the spec and findings together if one moved, then `template.py --check` |
| 4 | Another map and grenade | `/maps/<map>/<type>`, collect `a[href*="/nades/0"]`, repeat 1–2 | same template, or the differences written into findings |
| 5 | Against our render | journey 2's sheet beside `frames.png` from `nades-lineup-camera`'s `lineup.py` for the same `setpos`/`setang` and technique | the differences listed in findings' last table, phase by phase |

Last exercised: journeys 1 and 2 on 2026-11-02 against the Cloud9 interz Ancient smoke (macOS, built-in browser):
`info.json` matched the page and the 53-frame sheet showed every phase. Journey 3 ran the same day with the template's starts (0.1, 2.1, 3.2, 3.3, 6.5, 9.7, 10.3, 16.5 s): it moved the
spot's start from 0.2 to 0.1 s and the zoom's from 3.4 to 3.3 s. Journey 4 was done by hand that day
while writing the findings (the MIBR Ancient smoke and the MongolZ Mirage flash). Journey 5 has not been run.
