import { useState } from "react";
import Reports from "./Reports";
import ItemReport from "./ItemReport";
import "./Statistics.css";

interface Props {
  clockFormat: "auto" | "12h" | "24h";
  systemLocale: string;
}

export default function Statistics({ clockFormat, systemLocale }: Props) {
  const [tab, setTab] = useState<"trade" | "item">("trade");
  const [dateRange, setDateRange] = useState<number | "all">(30);

  return (
    <div className="statistics">
      <div className="stat-sub-tabs">
        <button className={tab === "trade" ? "active" : ""} onClick={() => setTab("trade")}>
          Trade Report
        </button>
        <button className={tab === "item" ? "active" : ""} onClick={() => setTab("item")}>
          Item Report
        </button>
      </div>
      {tab === "trade" ? <Reports dateRange={dateRange} onDateRangeChange={setDateRange} clockFormat={clockFormat} systemLocale={systemLocale} /> : <ItemReport />}
    </div>
  );
}
