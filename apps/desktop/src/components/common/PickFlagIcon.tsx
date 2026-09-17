import type { PickLabel } from "@/types";

interface PickFlagIconProps {
  value: PickLabel;
  size?: number;
  className?: string;
}

export function PickFlagIcon({ value, size = 15, className }: PickFlagIconProps) {
  return (
    <span
      className={`pick-flag-icon pick-flag-icon--${value}${className ? ` ${className}` : ""}`}
      aria-hidden="true"
    >
      <svg
        width={Math.round(size * 1.2)}
        height={size}
        viewBox="0 0 20 16"
        fill="none"
        xmlns="http://www.w3.org/2000/svg"
      >
        <path
          className="pick-flag-icon__outline"
          d="M2.5 3.15C6.9 1.45 11 4.35 17.5 2.35V11.65C11 13.65 6.9 10.75 2.5 12.45V3.15Z"
          stroke="currentColor"
          strokeWidth="1.6"
          strokeLinejoin="round"
        />
        <PickFlagMark value={value} />
      </svg>
    </span>
  );
}

function PickFlagMark({ value }: { value: PickLabel }) {
  if (value === "rejected") {
    return (
      <path
        d="M7.75 5.55L12.25 9.85M12.25 5.55L7.75 9.85"
        className="pick-flag-icon__mark"
      />
    );
  }

  if (value === "pending") {
    return <path d="M7.75 7.7H12.25" className="pick-flag-icon__mark" />;
  }

  return (
    <path
      d="M7.65 7.7L9.35 9.35L12.35 5.75"
      className="pick-flag-icon__mark"
    />
  );
}
