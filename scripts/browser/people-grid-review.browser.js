// On a Vite page in an isolated session:
// $probeCode = [Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes((Get-Content -Raw scripts/browser/people-grid-review.browser.js)))
// agent-browser --session <name> eval --base64 $probeCode
// Exercises the real App, Grid and IPC wrapper with controlled native responses.
// This verifies UI selection and submitted batches, not native persistence/media.
(async () => {
  const source = await (await fetch('/src/App.tsx')).text();
  const version = source.match(/react\.js(\?v=[a-z0-9]+)/)[1];
  const { default: React } = await import(`/node_modules/.vite/deps/react.js${version}`);
  const { default: ReactDOM } = await import(`/node_modules/.vite/deps/react-dom_client.js${version}`);
  const { QueryClient, QueryClientProvider } = await import(`/node_modules/.vite/deps/@tanstack_react-query.js${version}`);
  const { App } = await import('/src/App.tsx');
  const { useWorkspaceStore } = await import('/src/store.ts');
  const savedState = useWorkspaceStore.getState();
  const savedStorage = { ...localStorage };
  const savedInternals = window.__TAURI_INTERNALS__;
  const savedEvents = window.__TAURI_EVENT_PLUGIN_INTERNALS__;
  const session = { id: 'people-grid-review', rootPath: '/regression/people', displayName: 'people', openedAtMs: 0 };
  const assets = [1, 2, 3].map(n => ({ id: `photo${n}`, path: `${session.rootPath}/${n}.jpg`, name: `${n}.jpg`, extension: 'jpg', kind: 'jpeg', sizeBytes: 1, modifiedAtMs: 1, hasSidecar: false }));
  const person = { id: 'person', displayName: '测试人物', revision: 1, referenceInstanceIds: [], tagId: null };
  const members = assets.map((a, i) => ({ id: `instance${i}`, assetPath: a.path, sourceRevision: '1:1', sourceIdentityRevision: 'source', revision: 1, faceBox: [.1, .1, .2, .2], bodyBox: null, needsReview: false, personId: person.id, decision: 'pending', score: .8 }));
  const other = { ...members[0], id: 'other-person', personId: null, decision: null };
  const workspace = { folderPath: session.rootPath, revision: 'one', groups: [{ id: 'person:person', personId: person.id, members, cover: null }, { id: 'anonymous', personId: null, members: [other], cover: null }], unknownCount: 1, knownCount: 1, noFaceCount: 0, unavailableCount: 0, hasAnalysis: true, notice: null };
  const tree = { sessionId: session.id, revision: 1, root: { entry: { path: session.rootPath, name: 'people', hasChildren: false }, expanded: false, children: [] } };
  const calls = [];
  let callbackId = 0;
  window.__TAURI_INTERNALS__ = {
    transformCallback: () => ++callbackId,
    unregisterCallback: () => {},
    invoke: async (command, args) => {
      if (command === 'plugin:event|listen') return args.handler;
      if (command === 'list_library_roots') return [session.rootPath];
      if (command === 'open_folder') return session;
      if (command === 'get_directory_tree' || command === 'set_directory_expanded') return tree;
      if (command === 'list_assets') return { items: assets, total: assets.length };
      if (command === 'list_global_people') return [person];
      if (command === 'get_folder_people_workspace') return workspace;
      if (command === 'get_person_tuple_asset') return assets.find(a => a.path === args.path);
      if (command === 'get_person_operation') return null;
      if (command === 'get_preview') throw new Error('No media in selection-only fixture');
      if (command === 'review_person_tuples') { calls.push(args.input); return args.input.tuples.length; }
      return [];
    },
  };
  window.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener: () => {} };
  localStorage.setItem('oxyviewer.workspace.v1', JSON.stringify({ activeRoot: session.rootPath, currentDirectories: {} }));
  useWorkspaceStore.setState({ search: '', tagIds: [], kind: undefined, minimumRating: undefined, colorLabels: [], pickLabels: [], view: 'grid', selectedIds: [], activeId: undefined, inspectorOpen: false, burstGroupingEnabled: false });
  const container = document.createElement('div');
  container.style.cssText = 'position:fixed;inset:0;z-index:99999;background:#101214';
  document.body.append(container);
  const root = ReactDOM.createRoot(container);
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const assert = (value, message) => { if (!value) throw new Error(message); };
  const waitFor = async predicate => {
    for (let i = 0; i < 150; i++) {
      if (predicate()) return;
      await new Promise(resolve => setTimeout(resolve, 20));
    }
    throw new Error(`UI condition timed out: ${container.textContent.slice(0, 1200)}`);
  };
  const button = text => [...container.querySelectorAll('button')].find(b => b.textContent.includes(text));
  const card = n => container.querySelectorAll('.asset-card')[n];
  const click = (element, modifiers = {}) => element.dispatchEvent(new MouseEvent('click', { bubbles: true, ...modifiers }));
  const quick = () => container.querySelector('[aria-label="看图确认"]');
  const count = () => quick()?.querySelector('[aria-label="测试人物已选实例数"]')?.textContent;
  try {
    root.render(React.createElement(QueryClientProvider, { client }, React.createElement(App)));
    await waitFor(() => card(2));
    click(container.querySelectorAll('[role=tab]')[1]);
    await waitFor(() => button('当前文件夹'));
    click(button('当前文件夹'));
    await waitFor(() => card(2));
    await waitFor(() => button('测试人物'));
    click(button('测试人物'));
    await waitFor(() => container.querySelector('[aria-label="实例归属审阅"]'));
    click(card(0));
    await waitFor(() => quick()?.textContent.includes('2 人'));
    click(card(1), { ctrlKey: true });
    await waitFor(() => count() === '2');
    assert(container.querySelectorAll('.asset-card.is-selected').length === 2, 'Grid must have two selected photos');
    click(card(2), { shiftKey: true });
    await waitFor(() => useWorkspaceStore.getState().activeId === assets[2].id && count() === '2');
    assert(useWorkspaceStore.getState().selectedIds.join(',') === 'photo2,photo3', 'Shift must select the anchored range');
    click(card(0), { ctrlKey: true });
    await waitFor(() => count() === '3');
    click(card(2), { ctrlKey: true });
    await waitFor(() => count() === '2');
    for (const [label, decision] of [['批量确认为 测试人物', 'belongs'], ['批量不属于', 'doesNotBelong'], ['批量暂缓', 'deferred']]) {
      await waitFor(() => button(label) && !button(label).disabled);
      click(button(label));
      await waitFor(() => calls.at(-1)?.decision === decision && !button(label).disabled);
      assert(calls.at(-1).tuples.map(t => t.id).join(',') === 'instance0,instance1', `${decision}: selection must exclude other photo and other person`);
      assert(calls.at(-1).workspaceRevision === 'one', 'retain revision validation');
      assert(useWorkspaceStore.getState().view === 'grid', 'batch must stay in Grid');
    }
    click(card(1), { ctrlKey: true });
    await waitFor(() => quick()?.textContent.includes('2 人'));
    assert(!quick().textContent.includes('批量确认'), 'single selection must restore individual review');
    return { passed: ['Grid Ctrl selection', 'Shift range', 'deselection count', 'batch confirm/exclude/defer', 'group isolation', 'workspace revision', 'single-photo review'], submitted: calls.map(c => ({ decision: c.decision, ids: c.tuples.map(t => t.id) })) };
  } finally {
    root.unmount(); client.clear(); container.remove();
    useWorkspaceStore.setState(savedState);
    localStorage.clear(); Object.entries(savedStorage).forEach(([key, value]) => localStorage.setItem(key, value));
    window.__TAURI_INTERNALS__ = savedInternals;
    window.__TAURI_EVENT_PLUGIN_INTERNALS__ = savedEvents;
  }
})();
