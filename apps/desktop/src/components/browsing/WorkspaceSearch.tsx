import { useQuery } from "@tanstack/react-query";
import { useEffect, useMemo, useState } from "react";
import { listCustomTags } from "@/lib/api";
import { splitTagInput } from "@/lib/assets/tagSearch";
import type { MessageKey } from "@/lib/i18n";
import { useWorkspaceStore } from "@/store";
import { SearchField } from "./SearchField";


export function WorkspaceSearch({ t }: { t: (key: MessageKey) => string }) {
  const { search, setSearch, tagIds, setTagIds, tagMatch, setTagMatch, clearSearch, searchReset } = useWorkspaceStore();
  const [text, setText] = useState(search);
  const [recent, setRecent] = useState<number[]>([]);
  const tags = useQuery({ queryKey: ["custom-tags"], queryFn: listCustomTags });
  const input = splitTagInput(text);
  useEffect(() => { setText(useWorkspaceStore.getState().search); }, [searchReset]);
  useEffect(() => {
    if (splitTagInput(text).filename !== search) setText(search);
  }, [search, text]);
  useEffect(() => {
    if (!tags.isSuccess || !tags.data) return;
    const available = new Set(tags.data.map((tag) => tag.id));
    const valid = tagIds.filter((id) => available.has(id));
    if (valid.length !== tagIds.length) setTagIds(valid);
  }, [tags.data, tags.isSuccess, tagIds, setTagIds]);
  const suggestions = useMemo(() => {
    const candidates = (tags.data ?? []).filter((tag) => !tagIds.includes(tag.id));
    if (input.needle !== undefined) {
      const needle = input.needle.trim().toLocaleLowerCase();
      return candidates.filter((tag) => tag.path.replaceAll("|", " › ").toLocaleLowerCase().includes(needle)
        || tag.path.toLocaleLowerCase().includes(needle));
    }
    return text ? [] : recent.flatMap((id) => candidates.filter((tag) => tag.id === id)).slice(0, 6);
  }, [tags.data, tagIds, input.needle, text, recent]);
  const pathFor = (id: number) => tags.data?.find((tag) => tag.id === id)?.path.replaceAll("|", " › ") ?? `#${id}`;
  return <SearchField value={text} label={t("searchFilenameTags")} clearLabel={t("clearSearchTags")}
    onChange={(value) => { setText(value); setSearch(splitTagInput(value).filename); }}
    tokensLabel={t("selectedSearchTags")}
    tokens={tagIds.map((id) => ({ id: String(id), label: pathFor(id), removeLabel: `${t("removeSearchTag")}: ${pathFor(id)}` }))}
    onRemoveToken={(id) => setTagIds(tagIds.filter((selected) => String(selected) !== id))}
    onClear={() => { setText(""); clearSearch(); }}
    suggestions={suggestions.map((tag) => ({ id: String(tag.id), label: tag.path.replaceAll("|", " › ") }))}
    suggestionsLabel={input.needle === undefined ? t("recentSearchTags") : t("matchingSearchTags")}
    onSelectSuggestion={(value) => {
      const id = Number(value);
      setTagIds([...tagIds, id]);
      setRecent((previous) => [id, ...previous.filter((item) => item !== id)].slice(0, 6));
      setText(input.filename);
    }}
    panelContent={tagIds.length ? <div className="search-field__mode" role="group" aria-label={t("searchTagMode")}>
      {(["all", "any"] as const).map((mode) => <button key={mode} type="button"
        aria-pressed={tagMatch === mode} onClick={() => setTagMatch(mode)}>
        {t(mode === "all" ? "searchTagsAll" : "searchTagsAny")}
      </button>)}
    </div> : null}
    emptyMessage={tags.isError ? <div role="status">{t("searchTagsUnavailable")}
      <button type="button" onClick={() => void tags.refetch()}>{t("retry")}</button></div>
      : tags.isPending ? t("loading") : input.needle !== undefined ? t("noMatchingSearchTags") : t("searchTagHint")}
  />;
}
