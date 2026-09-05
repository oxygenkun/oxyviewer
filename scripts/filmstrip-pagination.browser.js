// Run in an isolated agent-browser session opened on the Vite dev server:
// PowerShell: agent-browser --session filmstrip eval (Get-Content -Raw scripts/filmstrip-pagination.browser.js)
// Exercises the real Loupe and CSS with delayed summary pages, without native media.
(async () => {
  const source = await (await fetch('/src/components/Loupe.tsx')).text();
  const version = source.match(/react\.js(\?v=[a-z0-9]+)/)[1];
  const { default: React } = await import(`/node_modules/.vite/deps/react.js${version}`);
  const { default: ReactDOM } = await import(`/node_modules/.vite/deps/react-dom_client.js${version}`);
  const { QueryClient, QueryClientProvider } = await import(`/node_modules/.vite/deps/@tanstack_react-query.js${version}`);
  const { Loupe } = await import('/src/components/Loupe.tsx');
  const { useWorkspaceStore } = await import('/src/store.ts');
  const { filmstripItemWidth, shouldFetchFilmstripPage } = await import('/src/lib/loupe.ts');
  const pause = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
  const assert = (value, message) => { if (!value) throw new Error(message); };
  const container = document.createElement('div');
  container.style.cssText = 'position:fixed;inset:0;z-index:99999;background:#101214';
  document.body.append(container);
  const root = ReactDOM.createRoot(container);
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const previousState = useWorkspaceStore.getState();
  const assets = Array.from({ length: 1473 }, (_, index) => ({
    id: `filmstrip-regression-${index}`, path: `/demo/${index}.jpg`, name: `${index}.jpg`,
    extension: 'jpg', kind: 'jpeg', sizeBytes: 1000, modifiedAtMs: 0, hasSidecar: false,
  }));
  let count = 750;
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
    }, 100);
  };
  function render() {
    root.render(React.createElement(QueryClientProvider, { client }, React.createElement(Loupe, {
      assets: assets.slice(0, count), total: assets.length, fetchNextPage,
      hasNextPage: count < assets.length, isFetchingNextPage: fetching,
      onAssetContextMenu: () => {}, t: (key) => key,
    })));
  }
  async function waitFor(predicate, message) {
    for (let attempt = 0; attempt < 100; attempt++) {
      if (predicate()) return;
      await pause(30);
    }
    assert(false, message);
  }
  const results = [];
  try {
    for (const reservedScrollbarHeight of [0, 10]) {
      for (const orientation of ['landscape', 'portrait']) {
        for (const height of [116, 180, 300]) {
          count = 750;
          calls = 0;
          fetching = false;
          useWorkspaceStore.setState({ activeId: assets[0].id, filmstripHeight: height, thumbnailOrientation: orientation });
          render();
          await pause(150);
          const strip = container.querySelector('.filmstrip');
          strip.scrollLeft = 0;
          // Headless Chrome uses overlay scrollbars. Also exercise the reduced
          // content height of a classic Windows horizontal scrollbar.
          strip.style.paddingBottom = `${6 + reservedScrollbarHeight}px`;
          await pause(50);
          assert(calls === 0, 'Opening the filmstrip eagerly fetched summaries');
          const boundary = container.querySelector('.filmstrip__unloaded');
          const boundaryLeft = boundary.getBoundingClientRect().left - strip.getBoundingClientRect().left;
          const jump = boundaryLeft - strip.clientWidth + 100;
          const estimatedRight = 9 + 750 * (filmstripItemWidth(height, orientation) + 5);
          const oldWouldFetch = shouldFetchFilmstripPage(jump, strip.clientWidth, estimatedRight);
          strip.scrollLeft = jump;
          await waitFor(() => count > 750 && !fetching, 'Scrolling into the unloaded region did not fetch a page');
          assert(calls === 1, 'Boundary scroll fetched unnecessary pages');
          // Jump across several pages. Follow-up pages must load even without another scroll event.
          strip.scrollLeft = strip.scrollWidth;
          await waitFor(() => count === assets.length && !fetching, 'Fast end jump stalled between pages');
          await pause(100);
          assert(!container.querySelector('.filmstrip__unloaded'), 'Unloaded spacer remained after the final page');
          results.push({ orientation, height, reservedScrollbarHeight, oldWouldFetch, pagesFetched: calls });
          strip.scrollLeft = 0;
        }
      }
    }
    assert(results.some((result) => !result.oldWouldFetch), 'Fixture did not reproduce the former boundary drift');
    return results;
  } finally {
    clearTimeout(timer);
    root.unmount();
    client.clear();
    container.remove();
    useWorkspaceStore.setState(previousState);
  }
})()
