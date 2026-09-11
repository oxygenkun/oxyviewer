// Open scripts/folder-thumbnail-probe.html via Vite's /@fs/ route in an isolated
// agent-browser session. PowerShell: pass (Get-Content -Raw this-file) to eval.
// Uses real browser decoding, PNG/Blob snapshots, Thumbnail and background work;
// the native transport is simulated. Desktop/native validation is separate.
(() => {
  const run = async () => {
    const code = await (await fetch('/src/components/Thumbnail.tsx')).text();
    const version = code.match(/react\.js(\?v=[a-z0-9]+)/)[1];
    const React = (await import(`/node_modules/.vite/deps/react.js${version}`)).default;
    const ReactDOM = (await import(`/node_modules/.vite/deps/react-dom_client.js${version}`)).default;
    const { QueryClient, QueryClientProvider } = await import(`/node_modules/.vite/deps/@tanstack_react-query.js${version}`);
    const { Thumbnail } = await import('/src/components/Thumbnail.tsx');
    const { BackgroundPreviewPreloader } = await import('/src/components/BackgroundPreviewPreloader.tsx');
    const { useBackgroundAssetPagination } = await import('/src/lib/useBackgroundAssetPagination.ts');
    const cache = await import('/src/lib/folderThumbnailCache.ts');
    const general = await import('/src/lib/browserImageCache.ts');
    const api = await import('/src/lib/api.ts');
    const pause = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
    const assert = (condition, message) => { if (!condition) throw new Error(message); };
    const waitFor = async (predicate, message, timeout = 60000) => {
      const until = performance.now() + timeout;
      while (performance.now() < until) {
        if (predicate()) return;
        await pause(25);
      }
      throw new Error(message);
    };
    const source = document.createElement('canvas');
    source.width = 160; source.height = 120;
    const context = source.getContext('2d');
    context.fillStyle = '#102238'; context.fillRect(0, 0, 160, 120);
    context.fillStyle = '#f581b6'; context.fillRect(0, 20, 160, 80);
    context.fillStyle = '#ffd166'; context.fillRect(20, 30, 35, 45);
    const sourceUrl = source.toDataURL('image/png');
    const large = document.createElement('canvas');
    large.width = 4000; large.height = 3000;
    large.getContext('2d').fillRect(0, 0, 4000, 3000);
    const largeUrl = large.toDataURL('image/png');
    large.width = 0; large.height = 0;
    const previousNative = window.__TAURI_INTERNALS__;
    let requests = 0, renewals = 0, released = 0, sequence = 0;
    const assets = Array.from({ length: 1100 }, (_, index) => ({
      id: `folder-${index}`, path: `/folder-probe/${index}.hif`, name: `${index}.hif`,
      kind: 'heif', extension: 'hif', sizeBytes: 10000, modifiedAtMs: 1, hasSidecar: false,
    }));
    window.__TAURI_INTERNALS__ = { invoke: async (command, args) => {
      if (command === 'get_preview') {
        requests++;
        const big = args.path.endsWith('large.jpg');
        const id = `probe-resource-${++sequence}`;
        return { path: args.path, sourceRevision: args.path, stateRevision: sequence,
          validAt: sequence, status: 'ready', level: 'thumbnail',
          result: { path: '/mock-cache/thumbnail', width: big ? 4000 : 160, height: big ? 3000 : 120,
            kind: big ? 'original' : 'embedded', renderLevel: 'thumbnail',
            ...(!big ? { geometry: { displaySize: { width: 6000, height: 4000 }, contentRect: { x: 0, y: 20, width: 160, height: 80 } } } : {}),
            resource: { resourceId: id, url: big ? largeUrl : sourceUrl, mediaType: 'image/png' },
          },
        };
      }
      if (command === 'renew_media_resource') { renewals++; return true; }
      if (command === 'release_media_resource') { released++; return; }
      if (command === 'cancel_preview_request') return true;
      throw new Error(`Unexpected native command: ${command}`);
    } };
    general.setBrowserImageResourceScope('probe-folder-a');
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const container = document.getElementById('probe');
    const root = ReactDOM.createRoot(container);
    const status = document.getElementById('status');
    let loaded = 250;
    function WarmFolder() {
      const [count, setCount] = React.useState(250);
      const [fetching, setFetching] = React.useState(false);
      loaded = count;
      const fetchNextPage = React.useCallback(async () => {
        setFetching(true);
        await pause(10);
        setCount((value) => Math.min(assets.length, value + 250));
        setFetching(false);
      }, []);
      useBackgroundAssetPagination({ scopeKey: 'probe-a', enabled: true, pageCount: Math.ceil(count / 250),
        hasNextPage: count < assets.length, isFetching: fetching, isError: false, fetchNextPage });
      const candidates = React.useMemo(() => assets.slice(0, count), [count]);
      return React.createElement(BackgroundPreviewPreloader, { assets: candidates });
    }
    const started = performance.now();
    root.render(React.createElement(WarmFolder));
    await waitFor(() => cache.getFolderThumbnailStats().count === assets.length, 'whole folder did not finish warming');
    const warmingMs = performance.now() - started;
    assert(loaded === 1100, 'background pagination did not fetch every summary');
    assert(requests === 1100, `expected 1100 native requests, got ${requests}`);
    for (let i = 0; i < 1600; i++) general.markBrowserImageReady(`generic-${i}`, { width: 1, height: 1 });
    assert(cache.getFolderThumbnail(assets[0]), 'first thumbnail was evicted by generic LRU');
    const shown = [...assets.slice(0, 6), ...assets.slice(-6)];
    const render = () => root.render(React.createElement(QueryClientProvider, { client }, shown.map((asset) =>
      React.createElement('div', { key: asset.id, className: 'probe-card' },
        React.createElement(Thumbnail, { asset }), React.createElement('span', null, asset.name)))));
    const callsBefore = { requests, renewals };
    const remountStart = performance.now();
    render();
    await waitFor(() => container.querySelectorAll('img').length === shown.length
      && [...container.querySelectorAll('img')].every((image) => image.complete && image.naturalWidth > 0), 'cached thumbnails did not paint');
    const remountMs = performance.now() - remountStart;
    assert(!container.querySelector('.thumbnail__fallback'), 'cached remount showed a placeholder');
    assert([...container.querySelectorAll('img')].every((image) => image.src.startsWith('blob:')), 'remount used an expiring native URL');
    assert(requests === callsBefore.requests && renewals === callsBefore.renewals, 'cached remount touched native media');
    assert(cache.getFolderThumbnail(assets[0]).geometry.contentRect.y === 20, 'thumbnail content geometry was lost');
    window.__folderThumbnailProbe = { state: 'painted', count: cache.getFolderThumbnailStats().count, warmingMs, remountMs, nativeCallsOnRemount: requests - callsBefore.requests };
    status.textContent = JSON.stringify(window.__folderThumbnailProbe);
    await new Promise((resolve) => { window.__folderThumbnailProbeContinue = resolve; });
    const big = { ...assets[0], id: 'large', path: '/folder-probe/large.jpg', name: 'large.jpg', kind: 'jpeg', extension: 'jpg' };
    await api.preloadAssetThumbnail(big);
    assert(cache.getFolderThumbnail(big).width === 512 && cache.getFolderThumbnail(big).height === 384, 'original JPEG was retained at full size');
    const retained = cache.getFolderThumbnailStats();
    const first = cache.getFolderThumbnail(assets[0]);
    await pause(20);
    root.render(null);
    await pause(20);
    general.setBrowserImageResourceScope('probe-folder-b');
    assert(cache.getFolderThumbnailStats().count === 0, 'old folder was retained after navigation');
    assert(first.image.getAttribute ? first.image.getAttribute('src') === '' : first.image.src === '', 'old decoded image reference was not released');
    const report = { count: retained.count, decodedBytes: retained.decodedBytes, requests, released,
      warmingMs, remountMs, nativeCallsOnRemount: requests - callsBefore.requests - 1,
      largeThumbnail: '512x384', previousFolderReleased: true, geometryPreserved: true };
    status.textContent = JSON.stringify(report);
    window.__folderThumbnailProbe = { state: 'passed', report };
    root.unmount();
    client.clear();
    window.__TAURI_INTERNALS__ = previousNative;
    return report;
  };
  window.__folderThumbnailProbe = { state: 'running' };
  void run().catch((error) => { window.__folderThumbnailProbe = { state: 'failed', error: String(error), stack: error.stack }; });
  return 'started';
})();
