import { useState } from "react";
import Reports from "./Reports";
import ItemReport from "./ItemReport";

const STATISTICS_CLASS = "flex min-h-0 flex-1 flex-col overflow-hidden";
const SUB_TABS_CLASS = "flex shrink-0 gap-[2px] border-b border-border px-3 py-1.5";
const SUB_TAB_CLASS = "cursor-pointer rounded-[4px] border px-3.5 py-[3px] text-[12px] transition-[background,color,border-color] duration-100";
const SUB_TAB_IDLE_CLASS = "border-[rgba(48,54,61,0.6)] bg-transparent text-muted hover:bg-[rgba(255,255,255,0.06)] hover:text-foreground";
const SUB_TAB_ACTIVE_CLASS = "border-accent bg-[rgba(56,139,253,0.15)] text-accent";

interface Props {
  clockFormat: "auto" | "12h" | "24h";
  systemLocale: string;
}

export default function Statistics({ clockFormat, systemLocale }: Props) {
  const [tab, setTab] = useState<"trade" | "item">("trade");
  const [dateRange, setDateRange] = useState<number | "all">(30);

  return (
    <div className={STATISTICS_CLASS}>
      <div className={SUB_TABS_CLASS}>
        <button className={`${SUB_TAB_CLASS} ${tab === "trade" ? SUB_TAB_ACTIVE_CLASS : SUB_TAB_IDLE_CLASS}`} onClick={() => setTab("trade")}>
          Trade Report
        </button>
        <button className={`${SUB_TAB_CLASS} ${tab === "item" ? SUB_TAB_ACTIVE_CLASS : SUB_TAB_IDLE_CLASS}`} onClick={() => setTab("item")}>
          Item Report
        </button>
      </div>
      {tab === "trade" ? <Reports dateRange={dateRange} onDateRangeChange={setDateRange} clockFormat={clockFormat} systemLocale={systemLocale} /> : <ItemReport />}
    </div>
  );
}
