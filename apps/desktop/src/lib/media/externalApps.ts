import { useQuery } from "@tanstack/react-query";
import { getExternalAppSettings } from "@/lib/api";
import type { ExternalAppSettings } from "@/types";

export const EXTERNAL_APPS_QUERY_KEY = ["external-apps"] as const;

export function useExternalAppSettings() {
  return useQuery({ queryKey: EXTERNAL_APPS_QUERY_KEY, queryFn: getExternalAppSettings, staleTime: Infinity });
}

export function removeExternalApplication(settings: ExternalAppSettings, id: string): ExternalAppSettings {
  const apps = settings.apps.filter((app) => app.id !== id);
  return { apps, defaultAppId: settings.defaultAppId === id ? apps[0]?.id ?? null : settings.defaultAppId };
}

export function moveExternalApplication(settings: ExternalAppSettings, index: number, delta: number): ExternalAppSettings {
  const next = index + delta;
  if (index < 0 || index >= settings.apps.length || next < 0 || next >= settings.apps.length) return settings;
  const apps = [...settings.apps];
  [apps[index], apps[next]] = [apps[next], apps[index]];
  return { ...settings, apps };
}
