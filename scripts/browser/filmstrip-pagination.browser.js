// Run in an isolated agent-browser session opened on the Vite dev server:
// PowerShell: agent-browser --session filmstrip eval (Get-Content -Raw scripts/browser/filmstrip-pagination.browser.js)
// Exercises the real virtualized Loupe with delayed summary pages, without native media.
(async () => {
  const source = await (await fetch('/src/components/loupe/Loupe.tsx')).text();
  const version = source.match(/react\.js(\?v=[a-z0-9]+)/)[1];
  const { default: React } = await import(`/node_modules/.vite/deps/react.js${version}`);
  const { default: ReactDOM } = await import(`/node_modules/.vite/deps/react-dom_client.js${version}`);
  const { QueryClient, QueryClientProvider } = await import(`/node_modules/.vite/deps/@tanstack_react-query.js${version}`);
  const { Loupe } = await import('/src/components/loupe/Loupe.tsx');
  const { useWorkspaceStore } = await import('/src/store.ts');
  const { filmstripItemWidth } = await import('/src/lib/preview/loupe.ts');
  const pause = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
  const assert = (value, message) => { if (!value) throw new Error(message); };
  const waitFor = async (predicate, message) => {
    for (let attempt = 0; attempt < 150; attempt++) {
      if (predicate()) return;
      await pause(20);
    }
    assert(false, message);
  };

  const container = document.createElement('div');
  container.style.cssText = 'position:fixed;inset:0;z-index:99999;background:#101214';
  document.body.append(container);
  const root = ReactDOM.createRoot(container);
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const previousState = useWorkspaceStore.getState();
  const assets = Array.from({ length: 1473 }, (_, index) => ({
    id: `filmstrip-regression-${index}`,
    path: `/demo/${index}.jpg`,
    name: `${index}.jpg`,
    extension: 'jpg',
    kind: 'jpeg',
    sizeBytes: 1000,
    modifiedAtMs: 0,
    hasSidecar: false,
  }));
  let count = 250;
  let calls = 0;
  let fetching = false;
  let timer;

  const fetchNextPage = () => {
    if (fetching) return;
    calls++;
    fetching = true;
    render();
    timer = setTimeout(() => {
      count = Math.min(assets.length, count + 250);
      fetching = false;
      render();
    }, 80);
  };
  function render() {
    root.render(React.createElement(QueryClientProvider, { client }, React.createElement(Loupe, {
      assets: assets.slice(0, count),
      total: assets.length,
      fetchNextPage,
      hasNextPage: count < assets.length,
      isFetchingNextPage: fetching,
      onAssetContextMenu: () => {},
      t: (key) => key,
    })));
  }

  try {
    useWorkspaceStore.setState({
      activeId: assets[0].id,
      filmstripHeight: 180,
      thumbnailOrientation: 'landscape',
    });
    render();
    await waitFor(() => container.querySelector('.filmstrip__track'), 'Filmstrip did not mount');
    assert(calls === 0, 'Opening the filmstrip eagerly fetched summaries');

    const strip = container.querySelector('.filmstrip');
    const stride = filmstripItemWidth(180, 'landscape') + 5;
    strip.scrollLeft = stride * 510;
    strip.dispatchEvent(new Event('scroll'));
    await waitFor(
      () => count >= 750 && !fetching,
      'A jump across an unfinished page did not load enough sequential pages',
    );
    await waitFor(
      () => container.querySelector('[data-filmstrip-asset-id="filmstrip-regression-510"]'),
      'The jumped-to asset did not replace its placeholder',
    );
    const mountedAt510 = container.querySelectorAll('[data-filmstrip-asset-id]').length;
    assert(mountedAt510 < 40, `Filmstrip mounted too many items near 510: ${mountedAt510}`);

    strip.scrollLeft = strip.scrollWidth;
    strip.dispatchEvent(new Event('scroll'));
    await waitFor(
      () => count === assets.length && !fetching,
      'A fast end jump stalled between sequential pages',
    );
    const mountedAtEnd = container.querySelectorAll('[data-filmstrip-asset-id]').length;
    assert(mountedAtEnd < 40, `Filmstrip mounted too many items at the end: ${mountedAtEnd}`);

    return {
      pagesFetched: calls,
      mountedAt510,
      mountedAtEnd,
      total: count,
    };
  } finally {
    clearTimeout(timer);
    root.unmount();
    client.clear();
    container.remove();
    useWorkspaceStore.setState(previousState);
  }
})()
