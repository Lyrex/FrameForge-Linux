import ItemImg from "../ItemImg";
import { ingredients, ingredientTitle } from "../lib/ingredients";
import { INGREDIENT_STATE_MARKS } from "../constants/ingredients";
import type { CraftPlan } from "../types/mastery";
import "./IngredientIcons.css";

export function IngredientIcons({ plan }: { plan: CraftPlan }) {
  const lines = ingredients(plan);
  if (lines.length === 0) return null;
  return (
    <div className="ingredients">
      {lines.map(line => (
        <span key={line.unique_name} className={`ingredient ingredient-${line.state}`} title={ingredientTitle(line)}>
          <ItemImg imageName={line.image_name ?? undefined} size={18} className="img ingredient-img"
            fallback={<span className="ingredient-fallback">{line.name.slice(0, 2)}</span>} />
          {INGREDIENT_STATE_MARKS[line.state] && <span className="ingredient-mark">{INGREDIENT_STATE_MARKS[line.state]}</span>}
        </span>
      ))}
    </div>
  );
}
