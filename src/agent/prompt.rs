use crate::db::Memory;

pub(crate) const MAX_FACT_VALUE_CHARS: usize = 500;

pub(crate) fn format_character_traits(traits: &[Memory]) -> String {
    let mut output = String::from("## character");
    for t in traits {
        let key = t.key.as_deref().unwrap_or("?");
        let value = truncate_chars(&t.content, MAX_FACT_VALUE_CHARS);
        output.push_str("\n- ");
        output.push_str(key);
        output.push_str(": ");
        output.push_str(&value);
    }
    output
}

pub(crate) fn format_known_facts(facts: &[Memory]) -> String {
    let mut grouped: Vec<(String, Vec<(String, String)>)> = Vec::new();

    for fact in facts {
        let category = fact.category.as_deref().unwrap_or("general").to_string();
        let key = fact.key.as_deref().unwrap_or("?").to_string();
        let value = truncate_chars(&fact.content, MAX_FACT_VALUE_CHARS);

        if let Some((_, entries)) = grouped.iter_mut().find(|(cat, _)| cat == &category) {
            entries.push((key, value));
        } else {
            grouped.push((category, vec![(key, value)]));
        }
    }

    let mut output = String::from("## known facts");
    for (category, entries) in grouped {
        output.push_str("\n\n### ");
        output.push_str(&category);
        for (key, value) in entries {
            output.push_str("\n- ");
            output.push_str(&key);
            output.push_str(": ");
            output.push_str(&value);
        }
    }

    output
}

pub(crate) fn format_recent_episodes(episodes: &[Memory]) -> String {
    let mut output = String::from("## recent memories");
    for ep in episodes {
        let date = ep.created_at.split(' ').next().unwrap_or(&ep.created_at);
        output.push_str("\n- [");
        output.push_str(date);
        output.push_str("] ");
        output.push_str(&truncate_chars(&ep.content, MAX_FACT_VALUE_CHARS));
    }
    output
}

pub(crate) fn format_pending_tasks(tasks: &[(i64, String)]) -> String {
    let mut output = String::from("## pending tasks");
    for (id, title) in tasks {
        output.push_str(&format!("\n- [id:{id}] {title}"));
    }
    output
}

fn truncate_chars(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }

    value.chars().take(max_chars).collect()
}
