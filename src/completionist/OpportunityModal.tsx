import { useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import ItemImg from "../ItemImg";
import { useModal } from "../shared/useModal";
import { IngredientIcon } from "../shared/IngredientIcons";
import { TAURI_COMMANDS } from "../constants/tauri";
import { CREDITS_PATH } from "../constants/ingredients";
import { blockerText } from "../constants/blockers";
import { dayAndClock, fmtClock, type ClockFormat } from "../lib/clockFormat";
import { availableText, ingredients, stateLabel } from "../lib/ingredients";
import { distinct, isBlueprint } from "../lib/craftPlan";
import { ACCESS_LABELS, acquisitionLines, actionText, alsoNeedsText, chanceText, sourceLines } from "./suggestions";
import type { Opportunity, Requirement } from "../types/mastery";
import type { RecipeComponent } from "../types/items";

type RecipeNode = Pick<RecipeComponent, "unique_name" | "name" | "components">;

interface LineProps {
  node: RecipeNode;
  rows: Requirement[];
  opportunity: Opportunity;
  now: number;
  depth: number;
  blueprint?: boolean;
}

/**
 * The icon row hides a part's own blueprint. Here it keeps a line of its own, because its relics
 * and drops are where a "Blueprint missing" part gets fixed. A line the plan never reached, because
 * its parent came out of stock, keeps its place and shows no counts.
 */
function Line({ node, rows, opportunity, now, depth, blueprint = false }: LineProps) {
  const [open, setOpen] = useState(false);
  const row = rows.find(r => r.unique_name === node.unique_name);
  const acquisition = node.components.length > 0 ? [] : acquisitionLines(opportunity, node.unique_name, now, blueprint);
  const expandable = node.components.length > 0 || acquisition.length > 0;
  const counts = row && availableText(row);
  return (
    <div style={{ marginLeft: depth * 16 }}>
      <div className="recipe-row" onClick={() => expandable && setOpen(o => !o)} style={{ cursor: expandable ? "pointer" : "default" }}>
        {expandable
          ? <span className="recipe-chevron">{open ? "▾" : "▸"}</span>
          : <span className="recipe-chevron recipe-chevron-leaf">·</span>}
        {row && <IngredientIcon line={row} />}
        <span className="recipe-name">{node.name}</span>
        {counts && <span className="recipe-counts">{counts}</span>}
        {row && <span className={`opp-state ingredient-${row.state}`}>{stateLabel(row)}</span>}
      </div>
      {open && distinct(node.components).map(child => (
        <Line key={child.unique_name} node={child} rows={rows} opportunity={opportunity} now={now} depth={depth + 1} />
      ))}
      {open && acquisition.map(text => <div key={text} className="opp-acquisition">{text}</div>)}
    </div>
  );
}

const leaf = (r: Requirement): RecipeNode => ({ unique_name: r.unique_name, name: r.name, components: [] });

interface Props {
  opportunity: Opportunity;
  notes?: string[];
  nowMs: number;
  clockFormat: ClockFormat;
  onClose: () => void;
}

export function OpportunityModal({ opportunity: o, notes = [], nowMs, clockFormat, onClose }: Props) {
  const modal = useModal(onClose);
  const [recipe, setRecipe] = useState<RecipeComponent[] | null>(null);
  // An owned copy's plan is the Forma it still needs, which no recipe tree frames, so it renders flat.
  const tree = !!o.craft && !o.owned;
  useEffect(() => {
    if (!tree) return;
    let stale = false;
    invoke<RecipeComponent[]>(TAURI_COMMANDS.GET_RECIPE, { uniqueName: o.unique_name })
      .then(r => { if (!stale) setRecipe(r); })
      .catch(() => { if (!stale) setRecipe([]); });
    return () => { stale = true; };
  }, [tree, o.unique_name]);

  const now = Math.floor(nowMs / 1000);
  const readyAt = o.build_completion_ms == null ? undefined : fmtClock(Math.floor(o.build_completion_ms / 1000), clockFormat);
  const source = sourceLines(o, nowMs, readyAt);
  const blockers = [...o.blockers.map(blockerText), ...notes];
  const rows = useMemo(() => o.craft ? ingredients(o.craft) : [], [o.craft]);
  const credits = rows.find(r => r.unique_name === CREDITS_PATH);
  const lineProps = { rows, opportunity: o, now, depth: 0 };
  return (
    <dialog className="craft-modal-overlay" {...modal}>
      <div className="craft-modal" onClick={e => e.stopPropagation()}>
        <div className="craft-modal-header">
          <ItemImg imageName={o.image_name ?? undefined} category={o.category} size={36} />
          <span className="craft-modal-title">{o.name}</span>
          <span className="mst-mr">{o.category}</span>
          {o.mastery_req != null && o.mastery_req > 0 && <span className="mst-mr">MR{o.mastery_req}</span>}
          <span className="mst-opp-action">{actionText(o, ms => dayAndClock(ms, clockFormat))}</span>
          {o.relic && (
            <span className={`mst-pill mst-pill-relic-${o.relic.coverage.kind}`}><span className="mst-pill-kind">Relics</span> {chanceText(o.relic.coverage)}</span>
          )}
          <span className={`mst-pill mst-pill-${o.access}`}>{ACCESS_LABELS[o.access]}</span>
          <button className="craft-detail-close" onClick={onClose}>✕</button>
        </div>
        <div className="craft-modal-body">
          {(source.length > 0 || blockers.length > 0 || o.purchase) && (
            <div className="opp-source">
              {source.map((text, i) => <div key={i}>{text}</div>)}
              {o.purchase && <div>Also needs {alsoNeedsText(o)}</div>}
              {blockers.length > 0 && <div className="mst-opp-blockers">{blockers.join(" · ")}</div>}
            </div>
          )}
          {credits && <Line node={leaf(credits)} {...lineProps} />}
          {tree ? (
            recipe == null ? <div className="empty-msg">Loading…</div>
            : recipe.length === 0 ? <div className="empty-msg">No recipe data.</div>
            : distinct(recipe).map(node => (
              <Line key={node.unique_name} node={node} blueprint={isBlueprint(node)} {...lineProps} />
            ))
          ) : rows.filter(r => r !== credits).map(r => <Line key={r.unique_name} node={leaf(r)} {...lineProps} />)}
        </div>
      </div>
    </dialog>
  );
}
