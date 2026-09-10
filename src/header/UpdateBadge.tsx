interface UpdateBadgeProps {
  version: string;
  onOpen: () => void;
}

export default function UpdateBadge({ version, onOpen }: UpdateBadgeProps) {
  return (
    <a className="update-badge" title={`v${version} available — click to install`} onClick={onOpen}>⬆ v{version}</a>
  );
}
