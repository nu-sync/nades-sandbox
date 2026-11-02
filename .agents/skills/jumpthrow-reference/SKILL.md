---
name: jumpthrow-reference
description: How jumpthrow.pro (the competitor lineup site) makes its grenade videos — the shot template, timings, camera and what each nade page carries — and how to re-observe it frame by frame in the built-in browser to compare with nades-sandbox's lineup videos. Use when matching or judging our lineup camera against jumpthrow, checking whether their template changed, or reading a jumpthrow nade page.
---

# jumpthrow.pro reference

jumpthrow.pro shows each pro grenade lineup as a looping MP4 recorded **inside CS2 itself** (first person, viewmodel
arms, no third-person player model), with two stills and the `setpos`/`setang` line. Our equivalent is
`lineup_render` ([nades-lineup-camera](../nades-lineup-camera/SKILL.md)). Read [what they do](references/findings.md)
for the template and how it differs from ours; run the [journeys](references/journeys.md) to re-observe it.

## Rules

- **Their footage stays local and uncommitted.** Frames, sheets and clips made from jumpthrow videos go in a scratch
  folder outside the repo (the session scratchpad, or `$TMPDIR/jumpthrow-<date>/`), never in the repo, `.runs/`
  included, and never uploaded or published. Describe them in words; share images only with the user, locally.
- Read only. No login, no votes, bookmarks or reports. Pages are public; nothing here needs an account.
- No downloading the MP4 (the `media.jumpthrow.pro` URL is signed and expires in about 5 minutes anyway). Observe it
  in the page.

## Tools

- The built-in browser (`mcp__Claude_Browser__*`): `navigate`, `javascript_tool`, `computer` screenshot. The page's
  viewport is 1024×768; screenshots come back in an 800×600 frame.
- [`scripts/sheet.js`](scripts/sheet.js): paste its whole text into `javascript_tool` on a nade page (wait about
  1.5 s after navigating first). It defines `window.jt`:
  - `jt.info()` → title, video URL, duration, size, map, type (e.g. `Smoke T`), airtime, buttons, console line,
    still URLs;
  - `jt.sheet(jt.range(t0, t1, step), cols)` → seeks the video to each time and draws a labelled contact sheet over
    the page; then take a `screenshot`. 6–8 columns suit 30–55 frames;
  - `jt.hide()` removes the overlay; `jt.seek(t)` for one frame.
- `ffmpeg` (`tile` filter) can stitch saved screenshots, but this build has no `drawtext`; the in-page sheet carries
  the labels.

## Gotchas found the first time

- The `<video>` has no `crossOrigin`, so a canvas it is drawn on is tainted: `toDataURL`/`getImageData` throw.
  Drawing to a visible canvas and screenshotting it works; that is what `sheet.js` does.
- `requestAnimationFrame` stalls while the browser pane is hidden; a seek that waits on it hangs `javascript_tool`
  (45 s timeout). `sheet.js` waits on `seeked` plus a 60 ms timer.
- After a timed-out script, the old script may still be running and hide the overlay; re-show it or reload.
- Listing pages: `https://jumpthrow.pro/maps/<map>/<type>` (smoke and flash seen; maps: ancient, anubis, cache,
  dust2, inferno, mirage, nuke, train, overpass). Nade links are `/nades/<uuid>`; a page also lists three
  "Similar Nades".

## Cleanup

`jt.hide()` or reload the page; nothing else is owned. Keep the evidence folder; it is outside the repo.
