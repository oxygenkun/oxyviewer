import { useEffect, useId, useState, type ButtonHTMLAttributes, type CSSProperties } from "react";
import { createPortal } from "react-dom";

export function FolderNameButton({ title, ...props }: ButtonHTMLAttributes<HTMLButtonElement>) {
  const tooltipId = useId();
  const [position, setPosition] = useState<CSSProperties>();

  useEffect(() => {
    if (!position) return;
    const hide = () => setPosition(undefined);
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") hide();
    };
    window.addEventListener("scroll", hide, true);
    window.addEventListener("resize", hide);
    window.addEventListener("keydown", onKeyDown);
    return () => {
      window.removeEventListener("scroll", hide, true);
      window.removeEventListener("resize", hide);
      window.removeEventListener("keydown", onKeyDown);
    };
  }, [position]);

  function show(button: HTMLButtonElement) {
    const bounds = button.getBoundingClientRect();
    const width = Math.min(420, window.innerWidth - 16);
    setPosition({
      left: Math.max(8, Math.min(bounds.left, window.innerWidth - width - 8)),
      maxWidth: width,
      bottom: window.innerHeight - bounds.top + 4,
    });
  }

  return (
    <>
      <button
        {...props}
        aria-describedby={position && title ? tooltipId : undefined}
        onMouseEnter={(event) => show(event.currentTarget)}
        onMouseLeave={() => setPosition(undefined)}
        onFocus={(event) => {
          if (event.currentTarget.matches(":focus-visible")) show(event.currentTarget);
        }}
        onBlur={() => setPosition(undefined)}
        onPointerDown={() => setPosition(undefined)}
      />
      {position && title ? createPortal(
        <div id={tooltipId} role="tooltip" className="folder-name-tooltip" style={position}>
          {title}
        </div>,
        document.body,
      ) : null}
    </>
  );
}
