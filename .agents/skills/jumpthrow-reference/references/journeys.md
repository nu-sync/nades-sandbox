# jumpthrow reference journeys

All run in the built-in browser with [`scripts/sheet.js`](../scripts/sheet.js) pasted into `javascript_tool` after
`navigate` and a 1.5 s wait. Evidence (screenshots, `info.json`) goes in a scratch folder outside the repo. A page
without a `<video>` or a site that refuses the browser is a gap, not a pass.

| # | Journey | Entry point | Success |
|---|---|---|---|
| 1 | Read a nade page | `navigate` `/nades/<uuid>`, paste `sheet.js`, `jt.info()` | video URL, duration, 1920×1080, map, type, airtime, buttons, `setpos`/`setang`, two still URLs; save as `info.json` |
| 2 | Whole-video timeline | `await jt.sheet(jt.range(0, jt.info().duration, 0.5), 8)`, then `screenshot` | one labelled sheet showing spot, aim, zoom, lineup, throw, chase, aftermath (the table in [findings](findings.md)) |
| 3 | Phase boundaries | `jt.sheet(jt.range(t0, t1, 0.1), 6)` round each cut (about 2.0–2.3, 3.1–3.5, 6.2–6.6, 9.4–10.6 s) | cut times within 0.1 s; compare with findings; update the findings if they moved |
| 4 | Another map and grenade | `/maps/<map>/<type>`, collect `a[href*="/nades/0"]`, repeat 1–2 | same template, or the differences written into findings |
| 5 | Against our render | journey 2's sheet beside `frames.png` from `nades-lineup-camera`'s `lineup.py` for the same `setpos`/`setang` and technique | the differences listed in findings' last table, phase by phase |

Last exercised: journeys 1 and 2 on 2026-11-02 against the Cloud9 interz Ancient smoke (macOS, built-in browser):
`info.json` matched the page and the 53-frame sheet showed every phase. Journeys 3 and 4 were done by hand that day
while writing the findings (the MIBR Ancient smoke and the MongolZ Mirage flash). Journey 5 has not been run.
