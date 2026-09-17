// Run with a Vite server in an isolated agent-browser session:
// agent-browser --session oxy-hif-geometry eval --stdin < scripts/browser/preview-geometry.browser.js
// Optional window.__sonyPreviewUrl supplies a real display-oriented 120x160 JPEG.
(async () => {
  const source = await (await fetch('/src/components/loupe/Loupe.tsx')).text();
  const version = source.match(/react\.js(\?v=[a-z0-9]+)/)[1];
  const { default: React } = await import(`/node_modules/.vite/deps/react.js${version}`);
  const { default: ReactDOM } = await import(`/node_modules/.vite/deps/react-dom_client.js${version}`);
  const { QueryClient, QueryClientProvider } = await import(`/node_modules/.vite/deps/@tanstack_react-query.js${version}`);
  const { Loupe } = await import('/src/components/loupe/Loupe.tsx');
  const { useWorkspaceStore } = await import('/src/store.ts');
  const { useImageProjectionStore, imageProjectionKey } = await import('/src/lib/projection/imageProjection.ts');
  const { preloadBrowserImage } = await import('/src/lib/cache/browserImageCache.ts');
  const pause = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
  const assert = (condition, message) => { if (!condition) throw new Error(message); };
  const near = (a, b) => Math.abs(a - b) < 0.1;
  const svg = (width, height, content) => 'data:image/svg+xml,' + encodeURIComponent(
    `<svg xmlns="http://www.w3.org/2000/svg" width="${width}" height="${height}">${content}</svg>`,
  );
  const preview = window.__sonyPreviewUrl ?? svg(120, 160,
    '<rect width="120" height="160" fill="black"/><rect x="7" width="106" height="160" fill="#bb7453"/>');
  const full = svg(4672, 7008, '<rect width="4672" height="7008" fill="#bb7453"/>');
  const asset = { id: 'geometry-regression', path: '/regression/geometry.HIF', name: 'geometry.HIF',
    kind: 'heif', extension: 'HIF', sizeBytes: 100, modifiedAtMs: 1, hasSidecar: false };
  const geometry = { displaySize: { width: 4672, height: 7008 },
    contentRect: { x: 7, y: 0, width: 106, height: 160 } };
  const previousState = useWorkspaceStore.getState();
  const previousRecords = useImageProjectionStore.getState().records;
  const container = document.createElement('div');
  container.style.cssText = 'position:fixed;inset:0;z-index:99999;background:#101214;display:grid';
  document.body.append(container);
  const root = ReactDOM.createRoot(container);
  const client = new QueryClient({ defaultOptions: { queries: { staleTime: Infinity, retry: false } } });
  const project = (url, padded) => useImageProjectionStore.setState({ records: {
    [imageProjectionKey(asset.path, 'thumbnail')]: {
      path: asset.path, level: 'thumbnail', status: 'ready', result: {
        path: url, url, width: padded ? 120 : 4672, height: padded ? 160 : 7008,
        kind: 'embedded', renderLevel: 'thumbnail', geometry: padded ? geometry : undefined,
      },
    },
  } });
  const rect = (selector) => container.querySelector(selector)?.getBoundingClientRect();
  const sameRect = (a, b) => ['x', 'y', 'width', 'height'].every((key) => near(a[key], b[key]));
  try {
    await preloadBrowserImage(preview);
    project(preview, true);
    client.setQueryData(['asset-details', asset.id], {});
    useWorkspaceStore.setState({ activeId: asset.id, focusAreasVisible: true });
    root.render(React.createElement(QueryClientProvider, { client }, React.createElement(Loupe, {
      assets: [asset], total: 1, fetchNextPage: () => {}, hasNextPage: false,
      isFetchingNextPage: false, onAssetContextMenu: () => {}, t: (key) => key,
    })));
    await pause(100);
    const before = rect('.loupe__render');
    assert(before?.height > 0 && near(before.width / before.height, 2 / 3), 'Preview must use full aspect before metadata');
    const content = rect('.loupe__render .thumbnail__content');
    assert(content && sameRect(content, before), 'Content rectangle must fill logical canvas');
    const image = container.querySelector('.loupe__render .thumbnail__content img');
    assert(image.naturalWidth === 120 && image.naturalHeight === 160, 'Raster dimensions must remain unchanged');
    const imageRect = image.getBoundingClientRect();
    assert(near(imageRect.x + imageRect.width * 7 / 120, content.x), 'Left padding must map outside the canvas');
    assert(near(imageRect.x + imageRect.width * 113 / 120, content.right), 'Right padding must map outside the canvas');
    client.setQueryData(['asset-details', asset.id], {
      width: 4672, height: 7008,
      focusInfo: { coordinateWidth: 4672, coordinateHeight: 7008,
        regions: [{ centerX: 1500, centerY: 1800, width: 200, height: 300 }] },
    });
    await pause(60);
    const focus = rect('.loupe__focus-frame');
    assert(focus && sameRect(before, rect('.loupe__render')), 'Metadata must not resize the preview');
    project(full, false);
    await pause(120);
    assert(container.querySelector('.loupe__render .thumbnail img')?.src === full, 'Full representation must be visible');
    assert(sameRect(before, rect('.loupe__render')), 'Full must not resize the canvas');
    assert(sameRect(focus, rect('.loupe__focus-frame')), 'Full must not move the focus region');
    return { raster: '120x160', logicalAspect: '2:3', metadataLayoutShift: 0, fullLayoutShift: 0, focusShift: 0 };
  } finally {
    root.unmount(); container.remove(); client.clear();
    useWorkspaceStore.setState(previousState);
    useImageProjectionStore.setState({ records: previousRecords });
  }
})();
