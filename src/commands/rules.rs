use crate::db::Database;
use crate::error;

pub fn run_rules(action: Option<crate::cli::RulesAction>) -> Result<(), error::Error> {
    let db = Database::open()?;

    match action {
        None => list_rules(&db),
        Some(crate::cli::RulesAction::Add { pattern }) => add_rule(&db, &pattern),
        Some(crate::cli::RulesAction::Rm { number }) => rm_rule(&db, number),
    }
}

fn list_rules(db: &Database) -> Result<(), error::Error> {
    let rules = db.list_approval_rules()?;
    if rules.is_empty() {
        println!("no approval rules");
        return Ok(());
    }
    for (i, rule) in rules.iter().enumerate() {
        println!("  {}. {}", i + 1, rule.pattern);
    }
    Ok(())
}

fn add_rule(db: &Database, pattern: &str) -> Result<(), error::Error> {
    let pattern = pattern.trim();
    if pattern.is_empty() {
        return Err(error::Error::Provider(
            "pattern cannot be empty".to_string(),
        ));
    }
    db.save_approval_rule(pattern)?;
    println!("added rule: {pattern}");
    Ok(())
}

fn rm_rule(db: &Database, number: usize) -> Result<(), error::Error> {
    if number < 1 {
        return Err(error::Error::Provider(
            "rule number must be >= 1".to_string(),
        ));
    }
    let rules = db.list_approval_rules()?;
    if number > rules.len() {
        return Err(error::Error::Provider(format!(
            "rule {number} not found (have {} rules)",
            rules.len()
        )));
    }
    let rule = &rules[number - 1];
    db.delete_approval_rule(rule.id)?;
    println!("removed rule: {}", rule.pattern);
    Ok(())
}
