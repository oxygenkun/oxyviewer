// Run against the Vite server in an isolated Windows agent-browser session.
// agent-browser --session oxy-loupe-switch eval (Get-Content -Raw scripts/loupe-switch.browser.js)
(async () => {
  const source = await (await fetch('/src/components/Loupe.tsx')).text();
  const version = source.match(/react\.js(\?v=[a-z0-9]+)/)[1];
  const { default: React } = await import(`/node_modules/.vite/deps/react.js${version}`);
  const { default: ReactDOM } = await import(`/node_modules/.vite/deps/react-dom_client.js${version}`);
  const { QueryClient, QueryClientProvider } = await import(`/node_modules/.vite/deps/@tanstack_react-query.js${version}`);
  const { Loupe } = await import('/src/components/Loupe.tsx');
  const { useWorkspaceStore } = await import('/src/store.ts');
  const { useImageProjectionStore, imageProjectionKey } = await import('/src/lib/imageProjection.ts');
  const { preloadBrowserImage } = await import('/src/lib/browserImageCache.ts');
  const assert = (value, message) => { if (!value) throw new Error(message); };
  const pause = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
  assert(/Windows/.test(navigator.userAgent), 'Windows HEIF renderer is required');
  const container = document.createElement('div');
  container.style.cssText = 'position:fixed;inset:0;z-index:99999;background:#101214';
  document.body.append(container);
  const root = ReactDOM.createRoot(container);
  const client = new QueryClient({ defaultOptions: { queries: { staleTime: Infinity, retry: false } } });
  const previousState = useWorkspaceStore.getState();
  const previousRecords = useImageProjectionStore.getState().records;
  const assets = ['red', 'green', 'blue'].map((color) => ({
    id: `switch-${color}`, path: `/regression/${color}.HIF`, name: `${color}.HIF`,
    kind: 'heif', extension: 'HIF', sizeBytes: 1000, modifiedAtMs: 0, hasSidecar: false,
  }));
  const urls = assets.map((_, index) => 'data:image/svg+xml,' + encodeURIComponent(
    `<svg xmlns="http://www.w3.org/2000/svg" width="160" height="120"><rect width="160" height="120" fill="${['red', 'green', 'blue'][index]}"/></svg>`,
  ));
  const errors = [];
  const originalError = console.error;
  console.error = (...args) => { errors.push(args.join(' ')); originalError(...args); };
  try {
    await Promise.all(urls.map((url) => preloadBrowserImage(url)));
    useImageProjectionStore.setState({ records: Object.fromEntries(assets.map((asset, index) => [
      imageProjectionKey(asset.path, 'thumbnail'),
      { path: asset.path, level: 'thumbnail', status: 'ready', result: {
        path: urls[index], url: urls[index], width: 160, height: 120, kind: 'embedded', renderLevel: 'thumbnail',
      } },
    ])) });
    for (const asset of assets) client.setQueryData(['asset-details', asset.id], { width: 160, height: 120 });
    useWorkspaceStore.setState({ activeId: assets[0].id });
    root.render(React.createElement(QueryClientProvider, { client }, React.createElement(Loupe, {
      assets, total: assets.length, fetchNextPage: () => {}, hasNextPage: false,
      isFetchingNextPage: false, onAssetContextMenu: () => {}, t: (key) => key,
    })));
    await pause(100);
    for (const index of [0, 1, 2, 1, 0, 2, 0, 1, 2]) {
      useWorkspaceStore.getState().select(assets[index].id);
      await pause(40);
      const layers = container.querySelectorAll('.loupe__render > .thumbnail');
      assert(layers.length === 1, `Selection ${index}: retained ${layers.length} thumbnail layers`);
      const image = layers[0].querySelector('img');
      assert(image?.getAttribute('src') === urls[index], `Selection ${index}: wrong displayed source`);
      assert(container.querySelectorAll('.loupe__render > canvas').length === 1, 'Expected one HEIF canvas');
    }
    assert(!errors.some((message) => /same key/.test(message)), 'Duplicate React keys');
    return { switches: 9, staleLayers: 0, wrongImages: 0 };
  } finally {
    root.unmount();
    container.remove();
    client.clear();
    useWorkspaceStore.setState(previousState);
    useImageProjectionStore.setState({ records: previousRecords });
    console.error = originalError;
  }
})();
