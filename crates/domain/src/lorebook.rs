use crate::models::{Scenario, WorldbookEntry};

/// Returns all `always_active` worldbook entries ordered by priority (for static system prompt).
pub fn always_active_worldbook(scenario: &Scenario) -> Vec<&WorldbookEntry> {
    let mut entries: Vec<&WorldbookEntry> = scenario
        .worldbook
        .iter()
        .filter(|e| e.always_active)
        .collect();
    entries.sort_by(|a, b| b.priority.cmp(&a.priority).then_with(|| a.id.cmp(&b.id)));
    entries
}

/// Returns dynamically keyword-triggered (non-always-active) lorebook entries.
pub fn match_dynamic_worldbook<'a>(
    scenario: &'a Scenario,
    corpus: &str,
    max_entries: usize,
) -> Vec<&'a WorldbookEntry> {
    let corpus_lower = corpus.to_lowercase();
    let mut matched: Vec<&WorldbookEntry> = scenario
        .worldbook
        .iter()
        .filter(|entry| {
            !entry.always_active
                && entry.keywords.iter().any(|kw| {
                    let kw = kw.trim();
                    !kw.is_empty() && corpus_lower.contains(&kw.to_lowercase())
                })
        })
        .collect();

    matched.sort_by(|a, b| b.priority.cmp(&a.priority).then_with(|| a.id.cmp(&b.id)));
    matched.truncate(max_entries);
    matched
}

/// Combined helper returning always-active entries followed by keyword-matched dynamic entries.
pub fn match_worldbook<'a>(
    scenario: &'a Scenario,
    corpus: &str,
    max_entries: usize,
) -> Vec<&'a WorldbookEntry> {
    let mut out = always_active_worldbook(scenario);
    let remaining = max_entries.saturating_sub(out.len());
    out.extend(match_dynamic_worldbook(scenario, corpus, remaining));
    out.truncate(max_entries);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::StarterCharacterDefaults;

    fn sample_scenario() -> Scenario {
        Scenario {
            id: "test".into(),
            name: "Test".into(),
            tagline: "Tag".into(),
            genre: "Fantasy".into(),
            theme: "ember".into(),
            dm_style: "Style".into(),
            initial_location: "Tavern".into(),
            initial_time: "Night".into(),
            opening_scene: "Opening".into(),
            opening_actions: vec![],
            player_defaults: StarterCharacterDefaults {
                max_hp: 10,
                max_san: 10,
                starter_items: vec![],
            },
            preset_roles: vec![],
            acts: vec![],
            endings: vec![],
            worldbook: vec![
                WorldbookEntry {
                    id: "always".into(),
                    title: "World".into(),
                    keywords: vec![],
                    always_active: true,
                    priority: 50,
                    content: "Always here".into(),
                },
                WorldbookEntry {
                    id: "secret".into(),
                    title: "Secret".into(),
                    keywords: vec!["酒窖".into(), "暗格".into()],
                    always_active: false,
                    priority: 90,
                    content: "Cellar secret".into(),
                },
                WorldbookEntry {
                    id: "unrelated".into(),
                    title: "Dragon".into(),
                    keywords: vec!["巨龙".into()],
                    always_active: false,
                    priority: 99,
                    content: "Dragon lore".into(),
                },
            ],
            map_nodes: vec![],
            map_edges: vec![],
            rules: Default::default(),
        }
    }

    #[test]
    fn matches_always_active_and_dynamic_separately() {
        let scenario = sample_scenario();
        assert_eq!(always_active_worldbook(&scenario).len(), 1);
        let dyn_entries = match_dynamic_worldbook(&scenario, "小明决定潜入地下酒窖搜查", 5);
        assert_eq!(dyn_entries.len(), 1);
        assert_eq!(dyn_entries[0].id, "secret");

        let combined = match_worldbook(&scenario, "小明决定潜入地下酒窖搜查", 5);
        let ids: Vec<&str> = combined.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(ids, vec!["always", "secret"]);
    }
}
