import { useEffect, useMemo, useState, type ReactNode } from "react";
import { invoke } from "@tauri-apps/api/core";
import ItemImg from "../ItemImg";
import ItemMarketPopup from "../market/ItemMarketPopup";
import { IngredientIcons } from "../shared/IngredientIcons";
import Filters from "./Filters";
import { TAURI_COMMANDS } from "../constants/tauri";
import { wfmSlugLookup } from "../utils";
import { fmtClock, type ClockFormat } from "../lib/clockFormat";
import {
  actionText, chanceText, costText, quoteText, remainingText, shownControls, visibleOpportunities, visiblePurchases,
  COMPARISON_OPTIONS, RELIC_GROUP_LABELS, RELIC_GROUP_ORDER, RESULT_OPTIONS, STAGE_LABELS, STAGE_ORDER,
  type Comparison, type MasteryControls, type Priced,
} from "./suggestions";
import type { Listing, MasteryOverview, Opportunity } from "../types/mastery";
import type { WfmItem } from "../types/market";
import type { WfmSession } from "../types/tauri";

const ACCESS_LABELS = { available: "Available", blocked: "Blocked", unknown: "Unknown access" } as const;

function Title({ opportunity: { category, mastery_req, name, action, needed_for } }: { opportunity: Opportunity }) {
  return (
    <div className="mst-opp-title">
      <span className="mst-name">{name}</span>
      {action === "level" && !!needed_for?.length && <span className="mst-mr">Needed for {needed_for.join(", ")}</span>}
      <span className="mst-mr">{category}</span>
      {mastery_req != null && mastery_req > 0 && (
        <span className="mst-mr" title={`Mastery Rank ${mastery_req} required`}>MR{mastery_req}</span>
      )}
    </div>
  );
}

function Remaining({ opportunity: { remaining_mastery, state, spend, forma } }: { opportunity: Opportunity }) {
  return (
    <span className={`mst-rank rank-${state}`} title={remaining_mastery == null ? "Remaining mastery unknown: no account observation yet" : "Remaining mastery"}>
      {remainingText(spend?.mastery ?? remaining_mastery, forma)}
    </span>
  );
}

interface RowProps {
  opportunity: Opportunity;
  nowMs: number;
  clockFormat: ClockFormat;
  notes?: string[];
  levelFirst?: boolean;
  children?: ReactNode;
}

export function OpportunityRow({ opportunity, clockFormat, notes = [], levelFirst = false, children }: RowProps) {
  const { access, craft, image_name, name, relic } = opportunity;
  const dayAndClock = (ms: number) => `${new Date(ms).toLocaleDateString(navigator.language, { month: "short", day: "numeric" })} ${fmtClock(Math.floor(ms / 1000), clockFormat)}`;
  return (
    <div className={`mst-opp mst-opp-${access}`} tabIndex={0}>
      <ItemImg imageName={image_name ?? undefined} fallback={<div className="img-fallback">{name[0]?.toUpperCase() ?? "?"}</div>} />
      <div className="mst-opp-main">
        <Title opportunity={opportunity} />
        {craft && <IngredientIcons plan={craft} />}
        {notes.length > 0 && <div className="mst-opp-blockers">{notes.join(" · ")}</div>}
      </div>
      <span className="mst-opp-action">{levelFirst ? `Level ${name} first` : actionText(opportunity, dayAndClock)}</span>
      {relic && (
        <span className={`mst-pill mst-pill-relic-${relic.coverage.kind}`} title="Chance of every missing relic part dropping from the relics you own, run solo at their current refinement">
          <span className="mst-pill-kind">Relics</span> {chanceText(relic.coverage)}
        </span>
      )}
      <span className={`mst-pill mst-pill-${access}`}>{ACCESS_LABELS[access]}</span>
      <Remaining opportunity={opportunity} />
      {children}
    </div>
  );
}

interface PurchaseRowProps {
  opportunity: Priced;
  comparison: Comparison;
  now: number;
  onOpen: (listing: Listing) => void;
}

function PurchaseRow({ opportunity, comparison, now, onOpen }: PurchaseRowProps) {
  const { access, image_name, name, purchase } = opportunity;
  const set = purchase.set;
  const cost = comparison === "full" ? purchase.full_purchase : purchase.cheapest_finish;
  const count = (part: { needed: number; short: number }) => comparison === "full" ? part.needed : part.short;
  const parts = purchase.parts.filter(p => count(p) > 0);
  return (
    <div className={`mst-opp mst-opp-${access}`} tabIndex={0}>
      <ItemImg imageName={image_name ?? undefined} fallback={<div className="img-fallback">{name[0]?.toUpperCase() ?? "?"}</div>} />
      <div className="mst-opp-main">
        <Title opportunity={opportunity} />
        <div className="mst-opp-listings">
          {set && (
            <button className={`mst-quote ${cost?.route === "set" ? "mst-quote-chosen" : ""}`} title="Market details" onClick={() => onOpen(set)}>
              {set.name}: {quoteText(set, now)}
            </button>
          )}
          {parts.map(p => (
            <button key={p.unique_name} className={`mst-quote ${cost?.route === "parts" ? "mst-quote-chosen" : ""}`} title="Market details" onClick={() => onOpen(p)}>
              {p.name}{count(p) > 1 ? ` ×${count(p)}` : ""}: {quoteText(p, now)}
            </button>
          ))}
        </div>
      </div>
      <span className="mst-opp-action">{costText(opportunity, comparison)}</span>
      <span className={`mst-pill mst-pill-${access}`}>{ACCESS_LABELS[access]}</span>
      <Remaining opportunity={opportunity} />
    </div>
  );
}

interface Props {
  overview: MasteryOverview;
  controls: MasteryControls;
  onChange: (patch: Partial<MasteryControls>) => void;
  nowMs: number;
  clockFormat: ClockFormat;
}

export default function WhatNext({ overview, controls, onChange, nowMs, clockFormat }: Props) {
  const [search, setSearch] = useState("");
  const [popup, setPopup] = useState<Listing | null>(null);
  const [wfmUsername, setWfmUsername] = useState<string | null>(null);
  const [wfmLookup, setWfmLookup] = useState<Map<string, string>>(new Map());
  const category = overview.categories.some(c => c.category === controls.category) ? controls.category : null;
  const shown = useMemo(() => shownControls({ ...controls, category }), [controls, category]);
  const visible = useMemo(
    () => visibleOpportunities(overview.opportunities, shown, search),
    [overview.opportunities, shown, search]);
  const purchases = useMemo(
    () => visiblePurchases(overview.opportunities, shown, search),
    [overview.opportunities, shown, search]);
  const groups = controls.result === "relics"
    ? RELIC_GROUP_ORDER.map(group => ({ key: group, label: RELIC_GROUP_LABELS[group], items: visible.filter(o => o.relic?.coverage.kind === group) }))
    : STAGE_ORDER.map(stage => ({ key: stage, label: STAGE_LABELS[stage], items: visible.filter(o => o.stage === stage) }));
  const listed = controls.result === "suggestions" || controls.result === "relics";
  const isFiltered = search !== "" || (!controls.easy && (category != null || controls.progress !== "all" || controls.availability !== "all"));
  const platinum = controls.result === "platinum";
  const suggestions = controls.result === "suggestions";

  // A quote can sit under a catalogue slug the market does not list (a prime
  // part "Blueprint"), so the popup opens the slug the item list knows.
  useEffect(() => {
    if (!platinum || wfmLookup.size > 0) return;
    invoke<WfmItem[]>(TAURI_COMMANDS.FETCH_WFM_ITEMS)
      .then(items => setWfmLookup(wfmSlugLookup(items)))
      .catch(() => {});
  }, [platinum, wfmLookup.size]);

  // The popup can place orders once the Trading tab has logged in.
  useEffect(() => {
    if (!popup || wfmUsername != null) return;
    invoke<WfmSession | null>(TAURI_COMMANDS.WFM_GET_SESSION)
      .then(session => { if (session) setWfmUsername(session[0]); })
      .catch(() => {});
  }, [popup, wfmUsername]);

  return (
    <>
      <div className="mst-tabs" role="group" aria-label="Result view">
        {RESULT_OPTIONS.map(v => (
          <button
            key={v.key}
            aria-pressed={controls.result === v.key}
            className={`mst-tab ${controls.result === v.key ? "active" : ""}`}
            onClick={() => onChange({ result: v.key })}
          >
            {v.label}
          </button>
        ))}
      </div>

      <Filters overview={overview} controls={controls} onChange={onChange} result={controls.result} search={search} onSearch={setSearch} />

      <div className="mst-body">
        {suggestions && overview.opportunities.length === 0 && (
          <div className="mst-empty">No suggestions yet: nothing owned or building with mastery left, and no known recipe or vendor route.</div>
        )}
        {controls.result === "relics" && !overview.opportunities.some(o => o.relic) && (
          <div className="mst-empty">No relic routes: nothing with mastery left is short a part that drops from a relic.</div>
        )}
        {listed && overview.opportunities.length > 0 && visible.length === 0 && (
          <div className="mst-empty">Nothing matches.</div>
        )}
        {listed && groups.filter(g => g.items.length > 0).map(({ key, label, items }) => (
          <section key={key} className="mst-group" aria-label={label}>
            <div className="mst-group-header">{label} <span className="mst-count">{items.length}</span></div>
            <div className="mst-opp-list">
              {items.map(o => <OpportunityRow key={o.unique_name} opportunity={o} nowMs={nowMs} clockFormat={clockFormat} />)}
            </div>
          </section>
        ))}
        {platinum && purchases.length === 0 && (
          <div className="mst-empty">
            {isFiltered ? "Nothing matches." : "Nothing to buy: no missing part or whole item with mastery left is sold on warframe.market."}
          </div>
        )}
        {platinum && purchases.length > 0 && (
          <section className="mst-group" aria-label="Purchases">
            <div className="mst-group-header">
              Cached estimates, ranked by {COMPARISON_OPTIONS.find(c => c.key === controls.comparison)?.label.toLowerCase()} <span className="mst-count">{purchases.length}</span>
            </div>
            <div className="mst-opp-list">
              {purchases.map(o => (
                <PurchaseRow key={o.unique_name} opportunity={o} comparison={controls.comparison} now={Math.floor(nowMs / 1000)} onOpen={setPopup} />
              ))}
            </div>
          </section>
        )}
      </div>

      {popup && (
        <ItemMarketPopup
          urlName={wfmLookup.get(popup.slug) ?? popup.slug}
          displayName={popup.name}
          onClose={() => setPopup(null)}
          isLoggedIn={wfmUsername != null}
          myUsername={wfmUsername ?? undefined}
        />
      )}
    </>
  );
}
