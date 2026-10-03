const UPDATE_BADGE =
  "inline-flex items-center gap-[4px] text-[10px] font-bold text-success bg-[rgba(63,185,80,.15)] border border-[rgba(63,185,80,.4)] rounded-[4px] px-[7px] py-[2px] shrink-0 tracking-[.02em] select-none whitespace-nowrap hover:bg-[rgba(63,185,80,.25)]";
const UPDATE_INSTALL =
  "bg-transparent border-0 text-inherit cursor-pointer [font:inherit] tracking-[inherit] p-0 focus-visible:outline focus-visible:outline-2 focus-visible:outline-accent focus-visible:outline-offset-2 disabled:cursor-wait";
const UPDATE_DISMISS =
  "bg-transparent border-0 text-[rgba(255,255,255,.65)] cursor-pointer text-[13px] leading-none px-[2px] py-0 flex items-center hover:text-white focus-visible:outline focus-visible:outline-2 focus-visible:outline-accent focus-visible:outline-offset-2";

interface UpdateBadgeProps {
  version: string;
  installing: boolean;
  onInstall: () => void;
  onDismiss: () => void;
}

export default function UpdateBadge({ version, installing, onInstall, onDismiss }: UpdateBadgeProps) {
  return (
    <span className={UPDATE_BADGE}>
      <button className={UPDATE_INSTALL} title={installing ? "Installing update…" : `v${version} is available — click to install`} onClick={onInstall} disabled={installing}>
        {installing ? "Installing…" : `v${version} ↑`}
      </button>
      {!installing && <button className={UPDATE_DISMISS} title="Dismiss" onClick={event => { event.stopPropagation(); onDismiss(); }}>×</button>}
    </span>
  );
}
