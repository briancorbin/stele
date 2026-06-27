use crate::ir::PluralTable;
use anyhow::{anyhow, Result};
use icu_locale_core::Locale;
use icu_plurals::{PluralCategory, PluralRules};

fn cat_name(c: PluralCategory) -> &'static str {
    match c {
        PluralCategory::Zero => "zero",
        PluralCategory::One => "one",
        PluralCategory::Two => "two",
        PluralCategory::Few => "few",
        PluralCategory::Many => "many",
        PluralCategory::Other => "other",
    }
}

/// Build a baked plural-category table for `tag`, using ICU4X (authoritative
/// CLDR data) as a *generate-time* oracle.
///
/// CLDR cardinal rules for an integer `n` depend only on `n` itself (for a few
/// small exact comparisons) and on `n % 10` / `n % 100`. So the category of any
/// non-negative integer is captured by two 100-entry tables:
///   - `small[n]`   for n in 0..100 (handles exact-value rules like Arabic 0/1/2)
///   - `modulo[n%100]` for n >= 100 (the periodic steady state)
///
/// The emitted runtime is then a pure table lookup — it never calls
/// `Intl.PluralRules`, which Hermes (React Native) doesn't implement.
pub fn build_plural_table(tag: &str) -> Result<PluralTable> {
    let loc: Locale = tag
        .parse()
        .map_err(|e| anyhow!("invalid locale tag '{tag}': {e}"))?;
    let rules = PluralRules::try_new_cardinal((&loc).into())
        .map_err(|e| anyhow!("no CLDR plural data for '{tag}': {e}"))?;

    let small: Vec<String> = (0u64..100)
        .map(|n| cat_name(rules.category_for(n)).to_string())
        .collect();
    let modulo: Vec<String> = (0u64..100)
        .map(|r| cat_name(rules.category_for(100 + r)).to_string())
        .collect();

    // Safety net: the small + mod-100 model holds for every CLDR cardinal rule,
    // but verify against the oracle so that a future rule which breaks the
    // assumption fails generation loudly instead of emitting wrong plurals.
    for n in 0u64..10_000 {
        let want = cat_name(rules.category_for(n));
        let got = if n < 100 {
            &small[n as usize]
        } else {
            &modulo[(n % 100) as usize]
        };
        if got != want {
            return Err(anyhow!(
                "plural model mismatch for '{tag}' at n={n} (baked {got}, ICU4X {want}) — \
                 this locale needs a richer model"
            ));
        }
    }

    let categories = rules
        .categories()
        .map(|c| cat_name(c).to_string())
        .collect();
    Ok(PluralTable {
        categories,
        small,
        modulo,
    })
}

#[cfg(test)]
mod tests {
    use super::build_plural_table;

    #[test]
    fn english_is_one_other() {
        let t = build_plural_table("en").unwrap();
        assert_eq!(t.small[0], "other");
        assert_eq!(t.small[1], "one");
        assert_eq!(t.small[21], "other");
        assert_eq!(t.categories, vec!["one", "other"]);
    }

    #[test]
    fn polish_one_few_many() {
        let t = build_plural_table("pl").unwrap();
        assert_eq!(t.small[1], "one");
        assert_eq!(t.small[2], "few");
        assert_eq!(t.small[5], "many");
        assert_eq!(t.small[22], "few");
        assert_eq!(t.modulo[12], "many"); // 112
        assert_eq!(t.modulo[22], "few"); // 122
        assert_eq!(t.categories, vec!["one", "few", "many", "other"]);
    }

    #[test]
    fn arabic_exact_rules_dont_fire_at_modulo() {
        let t = build_plural_table("ar").unwrap();
        assert_eq!(t.small[0], "zero");
        assert_eq!(t.small[1], "one");
        assert_eq!(t.small[2], "two");
        // 100, 101, 102 are Other — the exact 0/1/2 rules must not fire on the
        // mod-100 steady state. This is the case a naive lookup table gets wrong.
        assert_eq!(t.modulo[0], "other");
        assert_eq!(t.modulo[1], "other");
        assert_eq!(t.modulo[2], "other");
    }
}
