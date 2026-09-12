import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useState } from "react";
import { getRawDecoderStatus, openRawDecoderInstallPage } from "../lib/api";
import { regenerateRawFull } from "../lib/rawDecoder";
import type { MessageKey } from "../lib/i18n";
import type { AssetSummary } from "../types";

const DISMISSED_KEY = "oxyviewer.rawDecoderHintDismissed";
interface Props {
  asset?: AssetSummary;
  compact?: boolean;
  pending?: boolean;
  failed?: boolean;
  t: (key: MessageKey) => string;
}

export function RawDecoderPanel({ asset, compact = false, pending = false, failed = false, t }: Props) {
  const client = useQueryClient();
  const path = asset?.path;
  const key = ["raw-decoder", path ?? "settings"];
  const [installOpened, setInstallOpened] = useState(false);
  const [refreshed, setRefreshed] = useState(false);
  const [copyFailed, setCopyFailed] = useState(false);
  const [dismissed, setDismissed] = useState(() => {
    try { return localStorage.getItem(DISMISSED_KEY) === "1"; } catch { return false; }
  });
  const status = useQuery({
    queryKey: key,
    queryFn: () => getRawDecoderStatus(path),
    staleTime: 30_000,
    refetchInterval: (query) => compact && pending && !query.state.data?.attempt ? 1000 : false,
  });
  const refresh = useMutation({
    mutationFn: () => getRawDecoderStatus(path, true),
    onSuccess: (value) => { client.setQueryData(key, value); setRefreshed(true); },
  });
  const refreshAfterInstall = refresh.mutate;
  useEffect(() => {
    if (!installOpened) return;
    const onFocus = () => refreshAfterInstall();
    window.addEventListener("focus", onFocus);
    return () => window.removeEventListener("focus", onFocus);
  }, [installOpened, refreshAfterInstall]);
  useEffect(() => {
    if (compact && !pending) void status.refetch();
  }, [compact, pending, path]); // Terminal full state gets the latest native attempt.

  const retry = useMutation({
    mutationFn: () => asset ? regenerateRawFull(asset, client) : Promise.resolve(),
  });
  const install = useMutation({
    mutationFn: openRawDecoderInstallPage,
    onSuccess: () => { setInstallOpened(true); setRefreshed(false); },
  });
  const value = status.data;
  const supported = value?.availability !== "unsupportedPlatform";
  const missing = value?.availability === "missing";
  const fileFallback = value?.attempt && value.attempt.state !== "ready";
  const busy = retry.isPending || refresh.isPending || install.isPending;
  const error = status.error ?? retry.error ?? refresh.error ?? install.error;
  if (compact && !failed && (dismissed || !fileFallback)) return null;
  if (compact && !supported) return null;
  const diagnostics = JSON.stringify(value, null, 2);
  const copy = async () => {
    try { await navigator.clipboard.writeText(diagnostics); setCopyFailed(false); }
    catch { setCopyFailed(true); }
  };
  return <section className={`raw-decoder ${compact ? "raw-decoder--compact" : ""}`} aria-label={t("rawDecoderSupport")}
    onPointerDown={(event) => event.stopPropagation()} onWheel={(event) => event.stopPropagation()}>
    <strong>{t("rawDecoderSupport")}</strong>
    {!compact && <p>{t("rawBundledHint")}</p>}
    {!value ? <p role="status">{t("rawChecking")}</p> : <>
      <p role="status">{t(value.availability === "available" ? "rawSystemAvailable"
        : missing ? "rawSystemMissing" : supported ? "rawSystemUnavailable" : "rawWindowsOnly")}</p>
      {failed ? <p>{t("rawFullFailed")}</p> : fileFallback && !missing ? <p>{t("rawFileFallback")}</p> : null}
      {supported && <div className="raw-decoder__actions">
        {(missing || value.availability === "unavailable" || (!compact && value.installAvailable)) && <>
          <button disabled={busy} onClick={() => install.mutate(false)}>{t("rawOpenStore")}</button>
          <button disabled={busy} onClick={() => install.mutate(true)}>{t("rawOfficialPage")}</button>
        </>}
        <button disabled={busy} onClick={() => refresh.mutate()}>{t("rawCheckAgain")}</button>
        {path && <button disabled={busy || pending} onClick={() => retry.mutate()}>{t("rawRetryFull")}</button>}
        <button onClick={() => void copy()}>{t("rawCopyDiagnostics")}</button>
      </div>}
      {installOpened && <p role="status">{t(refreshed && value.installAvailable ? "rawRestartHint" : "rawInstallReturnHint")}</p>}
      {!compact && value.codecs.map((codec) => <small key={codec.decoderId}>{codec.name} · {codec.version}</small>)}
    </>}
    {error && <p role="alert">{String(error)}</p>}
    {copyFailed && <textarea aria-label={t("rawCopyDiagnostics")} readOnly value={diagnostics} onFocus={(event) => event.currentTarget.select()} />}
    {compact && !failed && <button className="raw-decoder__dismiss" onClick={() => {
      setDismissed(true);
      try { localStorage.setItem(DISMISSED_KEY, "1"); } catch { /* Session dismissal still works. */ }
    }}>{t("rawDismissHint")}</button>}
  </section>;
}
