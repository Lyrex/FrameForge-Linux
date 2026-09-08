interface UpdateBadgeProps {
  version: string;
  installing: boolean;
  onInstall: () => void;
  onDismiss: () => void;
}

export default function UpdateBadge({ version, installing, onInstall, onDismiss }: UpdateBadgeProps) {
  return (
    <span className="update-badge" title={installing ? "Installing update…" : `v${version} is available — click to install`} onClick={onInstall}>
      {installing ? "Installing…" : `v${version} ↑`}
      {!installing && <button className="update-badge-dismiss" title="Dismiss" onClick={event => { event.stopPropagation(); onDismiss(); }}>×</button>}
    </span>
  );
}
