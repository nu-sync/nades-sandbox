// Paste into the built-in browser's javascript_tool on a jumpthrow.pro/nades/<id> page.
// Defines window.jt: info(), seek(t), sheet(times, cols), range(t0, t1, step), hide().
// The video comes from media.jumpthrow.pro without CORS, so the canvas is tainted: frames can be drawn and
// screenshotted, never read back. Seeks wait on a timer, not requestAnimationFrame, which stalls while the
// browser pane is hidden.
(() => {
  const v = document.querySelector('video');
  const field = (label) => {
    const m = document.body.innerText.match(new RegExp(label + '\\n+([^\\n]+)'));
    return m ? m[1].trim() : null;
  };
  const seek = async (t) => {
    v.pause();
    v.currentTime = t;
    await new Promise((r) => v.addEventListener('seeked', r, { once: true }));
    await new Promise((r) => setTimeout(r, 60));
    return v.currentTime;
  };
  const canvas = () => {
    let c = document.getElementById('jt-sheet');
    if (!c) {
      c = document.createElement('canvas');
      c.id = 'jt-sheet';
      Object.assign(c.style, { position: 'fixed', left: '0', top: '0', width: '100vw', height: '100vh',
        zIndex: '2147483647', background: '#111' });
      document.body.appendChild(c);
    }
    return c;
  };
  const sheet = async (times, cols) => {
    const c = canvas();
    c.style.display = 'none';
    c.width = innerWidth * 2;
    c.height = innerHeight * 2;
    const g = c.getContext('2d');
    g.fillStyle = '#111';
    g.fillRect(0, 0, c.width, c.height);
    const rows = Math.ceil(times.length / cols);
    const tw = c.width / cols;
    const th = Math.min(tw * 9 / 16, c.height / rows);
    for (let i = 0; i < times.length; i++) {
      await seek(times[i]);
      const x = (i % cols) * tw, y = Math.floor(i / cols) * th;
      g.drawImage(v, x + 1, y + 1, th * 16 / 9 - 2, th - 2);
      g.fillStyle = 'rgba(0,0,0,.7)';
      g.fillRect(x + 2, y + 2, 78, 22);
      g.fillStyle = '#ff0';
      g.font = 'bold 18px monospace';
      g.fillText(times[i].toFixed(2) + 's', x + 5, y + 19);
    }
    c.style.display = 'block';
    return { frames: times.length, rows, cols };
  };
  const range = (t0, t1, step) => {
    const out = [];
    for (let t = t0; t <= Math.min(t1, v.duration - 0.02) + 1e-9; t += step) out.push(+t.toFixed(3));
    return out;
  };
  window.jt = {
    info: () => ({
      title: document.title,
      video: v.currentSrc.split('?')[0],
      duration: v.duration, width: v.videoWidth, height: v.videoHeight, loop: v.loop, muted: v.muted,
      map: field('Map'), type: field('Type'), airtime: field('Airtime'),
      buttons: (document.body.innerText.match(/Buttons\n([\s\S]*?)\nAirtime/) || [])[1]?.split('\n').filter(Boolean),
      console: (document.body.innerText.match(/(setpos[^\n]*)\n(setang[^\n]*)/) || []).slice(1),
      stills: [...document.querySelectorAll('img')].map((i) => i.src.split('?')[0]).filter((s) => /lineup/.test(s)),
    }),
    seek, sheet, range,
    hide: () => { const c = document.getElementById('jt-sheet'); if (c) c.style.display = 'none'; },
  };
  return v ? { ready: true, duration: v.duration } : { ready: false, reason: 'no <video> on this page' };
})();
