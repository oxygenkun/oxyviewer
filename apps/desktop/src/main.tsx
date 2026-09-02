import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App";
import { getPerfScenario } from "./lib/api";
import "./styles.css";

const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      staleTime: 10_000,
      retry: 1,
      refetchOnWindowFocus: false,
    },
  },
});

async function bootstrap() {
  // One extra IPC roundtrip at startup; undefined during normal app use.
  const perfScenario = await getPerfScenario();
  createRoot(document.getElementById("root")!).render(
    <StrictMode>
      <QueryClientProvider client={queryClient}>
        <App perfScenario={perfScenario} />
      </QueryClientProvider>
    </StrictMode>,
  );
}

void bootstrap();

