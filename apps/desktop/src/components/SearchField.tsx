import { Search, X } from "lucide-react";
import { useEffect, useId, useRef, useState, type ReactNode } from "react";

export interface SearchToken {
  id: string;
  label: string;
  removeLabel: string;
}

interface SearchFieldProps {
  value: string;
  onChange: (value: string) => void;
  label: string;
  clearLabel: string;
  tokens?: readonly SearchToken[];
  tokensLabel?: string;
  onRemoveToken?: (id: string) => void;
  onClear?: () => void;
  /** Omit until a suggestion source is available; ordinary search stays a searchbox. */
  suggestions?: readonly { id: string; label: string }[];
  suggestionsLabel?: string;
  panelContent?: ReactNode;
  emptyMessage?: ReactNode;
  onSelectSuggestion?: (id: string) => void;
}

/** Query intent belongs to the caller; composition, focus and highlighting stay local. */
export function SearchField({
  value, onChange, label, clearLabel, tokens = [], tokensLabel, onRemoveToken, onClear,
  suggestions, suggestionsLabel, onSelectSuggestion, panelContent, emptyMessage,
}: SearchFieldProps) {
  const inputRef = useRef<HTMLInputElement>(null);
  const composing = useRef(false);
  const [draft, setDraft] = useState<string | null>(null);
  const [open, setOpen] = useState(false);
  const [activeId, setActiveId] = useState<string | null>(null);
  const [pendingToken, setPendingToken] = useState<string | null>(null);
  const listId = useId();
  const expanded = open && suggestions !== undefined;
  const activeIndex = suggestions?.findIndex((item) => item.id === activeId) ?? -1;
  const isMac = /Mac|iPhone|iPad/.test(navigator.platform);

  useEffect(() => {
    const focusSearch = (event: KeyboardEvent) => {
      if (event.defaultPrevented || event.isComposing || event.altKey || event.shiftKey
        || event.key.toLowerCase() !== "k" || !(isMac ? event.metaKey : event.ctrlKey)) return;
      const target = event.target;
      if (target instanceof HTMLElement && target !== inputRef.current
        && target.closest('input, textarea, select, [contenteditable]:not([contenteditable="false"]), [role="textbox"]')) return;
      // Modal text editors and shortcut recorders own their keyboard input.
      if (document.querySelector('[aria-modal="true"], dialog[open]')) return;
      event.preventDefault();
      inputRef.current?.focus();
    };
    window.addEventListener("keydown", focusSearch);
    return () => window.removeEventListener("keydown", focusSearch);
  }, [isMac]);

  function selectSuggestion(id: string) {
    onSelectSuggestion?.(id);
    setOpen(false);
    setActiveId(null);
    inputRef.current?.focus();
  }

  return (
    <div className="search-field" onBlur={(event) => {
      if (!event.currentTarget.contains(event.relatedTarget)) {
        setOpen(false);
        setPendingToken(null);
      }
    }}>
      <Search size={15} aria-hidden="true" />
      {tokens.slice(0, 1).map((token) => (
        <button type="button" key={token.id}
          className={`search-field__token${pendingToken === token.id ? " is-pending" : ""}`}
          title={token.label} aria-label={token.removeLabel}
          onClick={() => {
            onRemoveToken?.(token.id);
            setPendingToken(null);
            inputRef.current?.focus();
          }}>
          <span>{token.label}</span><X size={12} aria-hidden="true" />
        </button>
      ))}
      {tokens.length > 1 ? <button type="button" className={pendingToken && pendingToken !== tokens[0]?.id ? "is-pending" : undefined} aria-label={`${tokensLabel ?? label} (+${tokens.length - 1})`} aria-expanded={expanded} onClick={() => {
        inputRef.current?.focus();
        setOpen(true);
      }}>+{tokens.length - 1}</button> : null}
      <input ref={inputRef} value={draft ?? value} aria-label={label} placeholder={label}
        role={suggestions === undefined ? "searchbox" : "combobox"}
        aria-autocomplete={suggestions === undefined ? undefined : "list"}
        aria-expanded={suggestions === undefined ? undefined : expanded}
        aria-controls={expanded ? listId : undefined}
        aria-activedescendant={expanded && activeIndex >= 0 ? `${listId}-${activeIndex}` : undefined}
        onFocus={() => setOpen(true)}
        onClick={() => setOpen(true)}
        onCompositionStart={(event) => {
          composing.current = true;
          setDraft(event.currentTarget.value);
          setActiveId(null);
          setPendingToken(null);
        }}
        onCompositionEnd={(event) => {
          composing.current = false;
          setDraft(null);
          onChange(event.currentTarget.value);
        }}
        onChange={(event) => {
          setPendingToken(null);
          setActiveId(null);
          setOpen(true);
          if (composing.current) setDraft(event.target.value);
          else if (event.target.value !== value) onChange(event.target.value);
        }}
        onKeyDown={(event) => {
          if (composing.current || event.nativeEvent.isComposing || event.keyCode === 229) return;
          if (event.key === "Escape") {
            if (open || pendingToken) event.stopPropagation();
            setOpen(false);
            setPendingToken(null);
          } else if (event.key === "Backspace" && !value && tokens.length) {
            event.preventDefault();
            const last = tokens[tokens.length - 1];
            if (pendingToken === last.id) {
              onRemoveToken?.(last.id);
              setPendingToken(null);
            } else setPendingToken(last.id);
          } else if ((event.key === "ArrowDown" || event.key === "ArrowUp") && suggestions?.length) {
            event.preventDefault();
            setOpen(true);
            const next = activeIndex < 0
              ? (event.key === "ArrowDown" ? 0 : suggestions.length - 1)
              : (activeIndex + (event.key === "ArrowDown" ? 1 : -1) + suggestions.length) % suggestions.length;
            setActiveId(suggestions[next].id);
          } else if (event.key === "Enter" && expanded && activeIndex >= 0) {
            event.preventDefault();
            selectSuggestion(suggestions![activeIndex].id);
          } else setPendingToken(null);
        }}
      />
      {(draft ?? value) || tokens.length ? (
        <button type="button" aria-label={clearLabel} title={clearLabel} onClick={() => {
          composing.current = false;
          setDraft(null);
          setPendingToken(null);
          setOpen(false);
          if (onClear) onClear();
          else onChange("");
          inputRef.current?.focus();
        }}><X size={13} aria-hidden="true" /></button>
      ) : null}
      <kbd aria-hidden="true">{isMac ? "⌘ K" : "Ctrl K"}</kbd>
      {expanded ? (
        <div className="search-field__suggestions">
          {panelContent}
          {tokens.length ? <div className="search-field__selected">{tokens.map((token) => (
            <button type="button" key={token.id} title={token.label} aria-label={token.removeLabel}
              onClick={() => { onRemoveToken?.(token.id); setPendingToken(null); inputRef.current?.focus(); }}>
              {token.label}<X size={12} aria-hidden="true" />
            </button>
          ))}</div> : null}
          {!suggestions.length ? emptyMessage : null}
          {suggestions.length ? <div className="search-field__heading">{suggestionsLabel ?? label}</div> : null}
          <div id={listId} role="listbox" aria-label={suggestionsLabel ?? label}>
          {suggestions.map((item, index) => (
            <div key={item.id} id={`${listId}-${index}`} role="option"
              aria-selected={index === activeIndex}
              onMouseDown={(event) => event.preventDefault()}
              onMouseMove={() => setActiveId(item.id)}
              onClick={() => selectSuggestion(item.id)}>{item.label}</div>
          ))}
          </div>
        </div>
      ) : null}
    </div>
  );
}
