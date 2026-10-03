import UpdateBadge from "./UpdateBadge";

const MASTERY_BADGE =
  "text-[11px] font-bold text-[#c0a060] bg-[rgba(192,160,96,.12)] border border-[rgba(192,160,96,.3)] rounded-[5px] px-[8px] py-[2px] shrink-0 tracking-[.03em]";
const PLAYER_NAME_BADGE =
  "text-[11px] font-semibold text-[#e0e0e0] bg-[rgba(255,255,255,.06)] border border-[rgba(255,255,255,.14)] rounded-[5px] px-[8px] py-[2px] shrink-0 tracking-[.02em]";
const BLOB_STATUS_BADGE =
  "text-[11px] font-semibold rounded-[5px] px-[9px] py-[2px] shrink-0 tracking-[.02em] text-[#3fb950] bg-[rgba(63,185,80,.10)] border border-[rgba(63,185,80,.30)]";

interface HeaderStatusBadgesProps {
  masteryRank: number | null;
  playerName: string | null;
  pendingUpdate: string | null;
  updateInstalling: boolean;
  inventoryLoaded: boolean;
  onInstallUpdate: () => void;
  onDismissUpdate: () => void;
}

export default function HeaderStatusBadges({
  masteryRank, playerName, pendingUpdate, updateInstalling, inventoryLoaded, onInstallUpdate, onDismissUpdate,
}: HeaderStatusBadgesProps) {
  return (
    <>
      {masteryRank !== null && <span className={MASTERY_BADGE} title="Mastery Rank">MR {masteryRank}</span>}
      {playerName && <span className={PLAYER_NAME_BADGE} title="Logged-in Warframe account">{playerName}</span>}
      {pendingUpdate && <UpdateBadge version={pendingUpdate} installing={updateInstalling} onInstall={onInstallUpdate} onDismiss={onDismissUpdate} />}
      {inventoryLoaded && <span className={BLOB_STATUS_BADGE} title="Inventory loaded from Warframe memory">Inventory Loaded</span>}
    </>
  );
}
