import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { Component, lazy, StrictMode, Suspense, useEffect, type ErrorInfo, type ReactNode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App";
import { closeDebugQueueWindow, getPerfScenario, openDebugQueueWindow } from "./lib/api";
import type { PerfScenario } from "./types";
import "./styles.css";

if (!__OXY_DEBUG__) {
  document.addEventListener("contextmenu", (event) => event.preventDefault());
}

const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      staleTime: 10_000,
      retry: 1,
      refetchOnWindowFocus: false,
    },
  },
});

const LazyDebugQueueDashboard = __OXY_DEBUG__
  ? lazy(() => import("./debug/DebugQueueDashboard").then((module) => ({ default: module.DebugQueueDashboard })))
  : undefined;

class DebugWindowErrorBoundary extends Component<{ children: ReactNode }, { error?: Error }> {
  state: { error?: Error } = {};

  static getDerivedStateFromError(error: Error) {
    return { error };
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    console.error("Failed to render queue diagnostics", error, info);
  }

  render() {
    if (this.state.error) {
      return (
        <main className="debug-window-status debug-window-error">
          <h1>Queue Observatory failed to load</h1>
          <pre>{this.state.error.message}</pre>
          <button onClick={() => void closeDebugQueueWindow()}>Close window</button>
        </main>
      );
    }
    return this.props.children;
  }
}

function RootView({ perfScenario, isQueueDebugWindow }: { perfScenario?: PerfScenario; isQueueDebugWindow: boolean }) {
  useEffect(() => {
    if (!__OXY_DEBUG__ || isQueueDebugWindow) return;
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.ctrlKey && event.shiftKey && event.code === "KeyD") {
        event.preventDefault();
        void openDebugQueueWindow();
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [isQueueDebugWindow]);
  if (isQueueDebugWindow && LazyDebugQueueDashboard) {
    return (
      <DebugWindowErrorBoundary>
        <Suspense fallback={<main className="debug-window-status">Loading Queue Observatory…</main>}>
          <LazyDebugQueueDashboard onClose={() => void closeDebugQueueWindow()} />
        </Suspense>
      </DebugWindowErrorBoundary>
    );
  }
  return (
    <>
      <App perfScenario={perfScenario} />
      {__OXY_DEBUG__ && (
        <button className="debug-queue-launcher" onClick={() => void openDebugQueueWindow()}>
          DEBUG QUEUES
        </button>
      )}
    </>
  );
}

async function bootstrap() {
  const windowContext = window as typeof window & { __OXY_QUEUE_DEBUG_WINDOW__?: boolean };
  let isQueueDebugWindow = __OXY_DEBUG__ && (
    new URLSearchParams(window.location.search).get("debug") === "queues"
    || windowContext.__OXY_QUEUE_DEBUG_WINDOW__ === true
  );
  if (__OXY_DEBUG__) {
    try {
      const { getCurrentWindow } = await import("@tauri-apps/api/window");
      isQueueDebugWindow ||= getCurrentWindow().label === "debug-queues";
    } catch {
      // Browser-only development has no Tauri window label; the query fallback remains available.
    }
  }

  // The diagnostics window must mount before making optional startup IPC calls.
  const perfScenario = isQueueDebugWindow ? undefined : await getPerfScenario().catch(() => undefined);
  createRoot(document.getElementById("root")!).render(
    <StrictMode>
      <QueryClientProvider client={queryClient}>
        <RootView perfScenario={perfScenario} isQueueDebugWindow={isQueueDebugWindow} />
      </QueryClientProvider>
    </StrictMode>,
  );
}

void bootstrap();
