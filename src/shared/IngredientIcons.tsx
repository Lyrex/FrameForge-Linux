import ItemImg from "../ItemImg";
import { iconLines, ingredientTitle } from "../lib/ingredients";
import { BLUEPRINT_ART, INGREDIENT_STATE_MARKS } from "../constants/ingredients";
import type { CraftPlan } from "../types/mastery";
import "./IngredientIcons.css";

export function IngredientIcons({ plan }: { plan: CraftPlan }) {
  const lines = iconLines(plan);
  if (lines.length === 0) return null;
  return (
    <div className="ingredients">
      {lines.map(line => (
        <span key={line.unique_name} className={`ingredient ingredient-${line.state}`} title={ingredientTitle(line)}>
          <span className={`ingredient-art${line.image_name === BLUEPRINT_ART ? " ingredient-art-blueprint" : ""}`}>
            <ItemImg imageName={line.image_name ?? undefined} size={22} className="img ingredient-img"
              fallback={<span className="ingredient-fallback">{line.name.slice(0, 2)}</span>} />
          </span>
          {INGREDIENT_STATE_MARKS[line.state] && <span className="ingredient-mark">{INGREDIENT_STATE_MARKS[line.state]}</span>}
        </span>
      ))}
    </div>
  );
}
