import * as Menu from "@radix-ui/react-dropdown-menu";
import { Check, ChevronRight, Ellipsis } from "lucide-react";
import { useState } from "react";
import type { FolderSort } from "@/lib/browse/folderOrdering";
import type { MessageKey } from "@/lib/i18n";
import styles from "./FolderSettingsMenu.module.css";

const SORT_OPTIONS = [
  ["import", "folderSortImport"],
  ["nameAscending", "folderSortNameAscending"],
  ["nameDescending", "folderSortNameDescending"],
] as const satisfies ReadonlyArray<readonly [FolderSort, MessageKey]>;

interface FolderSettingsMenuProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  disabled: boolean;
  folderSort: FolderSort;
  onFolderSortChange: (sort: FolderSort) => void;
  folderDragEnabled: boolean;
  onFolderDragEnabledChange: (enabled: boolean) => void;
  t: (key: MessageKey) => string;
}

export function FolderSettingsMenu({ open, onOpenChange, disabled, folderSort,
  onFolderSortChange, folderDragEnabled, onFolderDragEnabledChange, t }: FolderSettingsMenuProps) {
  const [restoredFocus, setRestoredFocus] = useState(false);
  return <Menu.Root open={open} onOpenChange={onOpenChange}>
    <Menu.Trigger className={styles.trigger} disabled={disabled}
      data-restored-focus={restoredFocus || undefined}
      onBlur={() => setRestoredFocus(false)}
      onKeyDown={() => setRestoredFocus(false)}
      title={t("folderActions")} aria-label={t("folderActions")}>
      <Ellipsis size={14} />
    </Menu.Trigger>
    <Menu.Portal>
      <Menu.Content className={styles.content} aria-label={t("folderActions")}
        side="right" align="start" sideOffset={6} collisionPadding={8} loop
        onCloseAutoFocus={() => setRestoredFocus(true)}
        onKeyDown={event => event.stopPropagation()}>
        <Menu.Sub>
          <Menu.SubTrigger className={styles.item}>
            <span /><span>{t("folderSort")}</span><ChevronRight size={13} />
          </Menu.SubTrigger>
          <Menu.Portal>
            <Menu.SubContent className={styles.content} aria-label={t("folderSort")}
              sideOffset={4} collisionPadding={8} loop onKeyDown={event => event.stopPropagation()}>
              <Menu.RadioGroup value={folderSort} onValueChange={value => {
                const option = SORT_OPTIONS.find(([sort]) => sort === value);
                if (option) onFolderSortChange(option[0]);
              }}>
                {SORT_OPTIONS.map(([value, label]) => <Menu.RadioItem key={value} value={value} className={styles.item}>
                  <span className={styles.indicator}><Menu.ItemIndicator><Check size={13} /></Menu.ItemIndicator></span>
                  <span>{t(label)}</span><span />
                </Menu.RadioItem>)}
              </Menu.RadioGroup>
            </Menu.SubContent>
          </Menu.Portal>
        </Menu.Sub>
        <Menu.Separator className={styles.separator} />
        <Menu.CheckboxItem className={styles.item} checked={folderDragEnabled}
          onCheckedChange={onFolderDragEnabledChange}>
          <span className={styles.indicator}><Menu.ItemIndicator><Check size={13} /></Menu.ItemIndicator></span>
          <span>{t("enableFolderDrag")}</span><span />
        </Menu.CheckboxItem>
      </Menu.Content>
    </Menu.Portal>
  </Menu.Root>;
}
