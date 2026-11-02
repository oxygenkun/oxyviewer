import { QueryClient } from "@tanstack/react-query";
import { expect, it } from "vitest";
import { refreshImportedTagQueries } from "./tagActions";

it("refreshes imported tag vocabulary, affected assignments and tag filters only", async () => {
  const client = new QueryClient();
  const changed = [
    ["custom-tags"], ["tag-sync-status"],
    ["asset-tag-assignments", ["/photos/a.HIF", "/photos/b.HIF"]],
    ["assets", "session", "/photos", { tagIds: [1] }],
  ];
  const unchanged = [
    ["asset-tag-assignments", ["/photos/b.HIF"]],
    ["assets", "session", "/photos", { minimumRating: 3 }],
    ["asset-details", "a"], ["preload-assets", "session", "/photos"],
  ];
  for (const key of [...changed, ...unchanged]) client.setQueryData(key, []);
  await refreshImportedTagQueries(client, new Set(["/photos/a.HIF"]));
  for (const key of changed) expect(client.getQueryState(key)?.isInvalidated).toBe(true);
  for (const key of unchanged) expect(client.getQueryState(key)?.isInvalidated).toBe(false);
});
