// Run in the native Vite WebView via agent-browser eval (Get-Content -Raw ...).
// Measures scrolling itself, independently of stop-to-thumbnail readiness.
window.__gridFrameResult = undefined;
window.__gridFrameRun = (async () => {
  const el = document.querySelector('.asset-scroll');
  if (!el) throw new Error('Grid scroll surface missing');
  el.scrollTop = 0;
  await new Promise(resolve => setTimeout(resolve, 500));
  const frames = [], tasks = [];
  let last;
  const observer = new PerformanceObserver(list => tasks.push(...list.getEntries().map(x => ({
    start: x.startTime, duration: x.duration,
  }))));
  observer.observe({ type: 'longtask' });
  const started = performance.now();
  await new Promise(resolve => {
    const step = now => {
      if (last !== undefined) frames.push(now - last);
      last = now;
      el.scrollTop = (el.scrollHeight - el.clientHeight) * Math.min(1, (now - started) / 5000);
      if (now - started < 5500) requestAnimationFrame(step);
      else resolve();
    };
    requestAnimationFrame(step);
  });
  observer.disconnect();
  frames.sort((a, b) => a - b);
  const result = {
    path: el.querySelector('.asset-card')?.title,
    scrollTop: el.scrollTop, maxScroll: el.scrollHeight - el.clientHeight,
    cards: el.querySelectorAll('.asset-card').length,
    placeholders: el.querySelectorAll('.asset-card-placeholder').length,
    frames: frames.length, p50: frames[Math.floor(frames.length * .5)],
    p95: frames[Math.floor(frames.length * .95)], maxFrame: frames.at(-1), tasks,
  };
  return window.__gridFrameResult = result;
})();
'started';
