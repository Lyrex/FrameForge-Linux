import UpdateBadge from "./UpdateBadge";

interface HeaderStatusBadgesProps {
  masteryRank: number | null;
  playerName: string | null;
  updateVersion: string | null;
  inventoryLoaded: boolean;
  onOpenUpdate: () => void;
}

export default function HeaderStatusBadges({
  masteryRank, playerName, updateVersion, inventoryLoaded, onOpenUpdate,
}: HeaderStatusBadgesProps) {
  return (
    <>
      {updateVersion && <UpdateBadge version={updateVersion} onOpen={onOpenUpdate} />}
      {masteryRank !== null && <span className="mastery-badge" title="Mastery Rank">MR {masteryRank}</span>}
      {playerName && <span className="player-name-badge" title="Logged-in Warframe account">{playerName}</span>}
      {inventoryLoaded && <span className="blob-status-badge blob-status-done" title="Inventory loaded from Warframe memory">Inventory Loaded</span>}
    </>
  );
}
