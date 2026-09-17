// Run on a Vite page in an isolated browser session. Uses the real App and IPC
// wrappers with controlled native responses; this checks UI, not NAS latency.
(async () => {
  const source = await (await fetch('/src/App.tsx')).text();
  const version = source.match(/react\.js(\?v=[a-z0-9]+)/)[1];
  const { default: React } = await import(`/node_modules/.vite/deps/react.js${version}`);
  const { default: ReactDOM } = await import(`/node_modules/.vite/deps/react-dom_client.js${version}`);
  const { QueryClient, QueryClientProvider } = await import(`/node_modules/.vite/deps/@tanstack_react-query.js${version}`);
  const { App } = await import('/src/App.tsx');
  const { useWorkspaceStore } = await import('/src/store.ts');
  const savedWorkspace = localStorage.getItem('oxyviewer.workspace.v1');
  const savedState = useWorkspaceStore.getState();
  const savedInternals = window.__TAURI_INTERNALS__;
  const savedEvents = window.__TAURI_EVENT_PLUGIN_INTERNALS__;
  const session = { id: 'startup-regression', rootPath: '/regression/2026', displayName: '2026', openedAtMs: 0 };
  const callbacks = new Map();
  const events = new Map();
  let id = 0;
  let resolvePage;
  let reads = 0;
  const progress = { sessionId: session.id, directory: session.rootPath, stage: 'enumerating', source: '', discoveredCount: 42,
    elapsedMs: 10, cacheMs: 1, enumerationMs: 9, attributesMs: 0, sortMs: 0 };
  const tree = { sessionId: session.id, revision: 1, root: { entry: { path: session.rootPath, name: '2026', hasChildren: false }, expanded: false, children: [] } };
  window.__TAURI_INTERNALS__ = {
    transformCallback: (callback) => { callbacks.set(++id, callback); return id; },
    unregisterCallback: (key) => callbacks.delete(key),
    invoke: async (command, args) => {
      if (command === 'plugin:event|listen') { events.set(args.event, args.handler); return args.handler; }
      if (command === 'plugin:event|unlisten') return;
      if (command === 'list_library_roots') return [session.rootPath];
      if (command === 'open_folder') return session;
      if (command === 'set_active_directory') return;
      if (command === 'get_directory_tree' || command === 'set_directory_expanded') return tree;
      if (command === 'list_assets') {
        reads++;
        if (reads > 1) return { items: [], total: 0, progress: { ...progress, source: 'memory', stage: 'ready' } };
        return new Promise((resolve) => { resolvePage = resolve; });
      }
      return [];
    },
  };
  window.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener: () => {} };
  localStorage.setItem('oxyviewer.workspace.v1', JSON.stringify({ activeRoot: session.rootPath, currentDirectories: {} }));
  useWorkspaceStore.setState({ search: '', kind: undefined, minimumRating: undefined, colorLabels: [], view: 'grid' });
  const container = document.createElement('div');
  container.style.cssText = 'position:fixed;inset:0;z-index:99999;background:#101214';
  document.body.append(container);
  const root = ReactDOM.createRoot(container);
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const assert = (value, message) => { if (!value) throw new Error(message); };
  const waitFor = async (predicate) => {
    for (let i = 0; i < 100; i++) {
      if (predicate()) return;
      await new Promise((resolve) => setTimeout(resolve, 20));
    }
    throw new Error('UI condition timed out');
  };
  const emit = (payload) => callbacks.get(events.get('directory-browse-progress'))({ payload });
  try {
    root.render(React.createElement(QueryClientProvider, { client }, React.createElement(App)));
    await waitFor(() => resolvePage && events.has('directory-browse-progress'));
    emit(progress);
    await waitFor(() => container.textContent.includes('已发现 42'));
    assert(!container.querySelector('.toolbar__title').textContent.includes('0 张照片'), 'loading must not claim zero photos');
    resolvePage({ items: [], total: 0, progress: { ...progress, stage: 'ready', source: 'snapshot' } });
    emit({ ...progress, stage: 'ready', source: 'snapshot' });
    await waitFor(() => container.querySelector('.workspace-browse-status'));
    assert(getComputedStyle(container.querySelector('.workspace-browse-status')).position === 'absolute', 'notice must not add a workspace grid row');
    emit({ ...progress, stage: 'stale', error: 'NAS offline' });
    await waitFor(() => container.textContent.includes('暂时无法核对目录'));
    emit({ ...progress, stage: 'updated' });
    await waitFor(() => reads === 2);
    assert(localStorage.getItem('oxyviewer.workspace.v1').includes(session.rootPath), 'background update must preserve directory selection');
    return { passed: ['loading count', 'snapshot notice', 'offline notice', 'background refetch', 'directory selection', 'workspace layout'] };
  } finally {
    root.unmount();
    client.clear();
    container.remove();
    useWorkspaceStore.setState(savedState);
    if (savedWorkspace === null) localStorage.removeItem('oxyviewer.workspace.v1');
    else localStorage.setItem('oxyviewer.workspace.v1', savedWorkspace);
    await new Promise((resolve) => setTimeout(resolve, 30));
    if (savedInternals === undefined) delete window.__TAURI_INTERNALS__;
    else window.__TAURI_INTERNALS__ = savedInternals;
    if (savedEvents === undefined) delete window.__TAURI_EVENT_PLUGIN_INTERNALS__;
    else window.__TAURI_EVENT_PLUGIN_INTERNALS__ = savedEvents;
  }
})();
