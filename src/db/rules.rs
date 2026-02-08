use crate::error::Error;

use super::Database;

#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalRule {
    pub id: i64,
    pub pattern: String,
}

impl Database {
    pub fn save_approval_rule(&self, pattern: &str) -> Result<(), Error> {
        tracing::debug!(pattern, "saving approval rule");
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO approval_rules (pattern) VALUES (?1)",
            [pattern],
        )?;
        Ok(())
    }

    #[allow(dead_code)]
    pub fn find_matching_rule(&self, command: &str) -> Result<Option<i64>, Error> {
        let rules = self.list_approval_rules()?;
        for rule in rules {
            if matches_rule(&rule.pattern, command) {
                return Ok(Some(rule.id));
            }
        }
        Ok(None)
    }

    pub fn list_approval_rules(&self) -> Result<Vec<ApprovalRule>, Error> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT id, pattern FROM approval_rules ORDER BY id")?;

        let rules = stmt
            .query_map([], |row| {
                Ok(ApprovalRule {
                    id: row.get(0)?,
                    pattern: row.get(1)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(rules)
    }

    #[allow(dead_code)]
    pub fn delete_approval_rule(&self, id: i64) -> Result<bool, Error> {
        let conn = self.conn.lock().unwrap();
        let rows = conn.execute("DELETE FROM approval_rules WHERE id = ?1", [id])?;
        Ok(rows > 0)
    }
}

/// matches a command against a rule pattern.
/// tokens are space-separated. `*` as trailing wildcard matches any remaining args.
/// `*` in a middle position matches exactly one token.
/// for commands with pipes/chains (|, &&, ||, ;), each sub-command must match.
#[allow(dead_code)]
fn matches_rule(pattern: &str, command: &str) -> bool {
    let sub_commands = split_subcommands(command);

    // every sub-command must match the pattern
    sub_commands
        .iter()
        .all(|sub| matches_single(pattern, sub.trim()))
}

#[allow(dead_code)]
fn split_subcommands(command: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0;
    let bytes = command.as_bytes();
    let len = bytes.len();
    let mut i = 0;

    while i < len {
        match bytes[i] {
            b'|' => {
                if i + 1 < len && bytes[i + 1] == b'|' {
                    // ||
                    parts.push(&command[start..i]);
                    i += 2;
                    start = i;
                } else {
                    // |
                    parts.push(&command[start..i]);
                    i += 1;
                    start = i;
                }
            }
            b'&' if i + 1 < len && bytes[i + 1] == b'&' => {
                // &&
                parts.push(&command[start..i]);
                i += 2;
                start = i;
            }
            b';' => {
                parts.push(&command[start..i]);
                i += 1;
                start = i;
            }
            _ => {
                i += 1;
            }
        }
    }

    if start < len {
        parts.push(&command[start..]);
    }

    parts
}

#[allow(dead_code)]
fn matches_single(pattern: &str, command: &str) -> bool {
    let pattern_tokens: Vec<&str> = pattern.split_whitespace().collect();
    let command_tokens: Vec<&str> = command.split_whitespace().collect();

    if pattern_tokens.is_empty() {
        return command_tokens.is_empty();
    }

    for (i, pat) in pattern_tokens.iter().enumerate() {
        let is_last = i == pattern_tokens.len() - 1;

        if *pat == "*" {
            if is_last {
                // trailing * matches everything remaining
                return true;
            }
            // middle * matches exactly one token
            if i >= command_tokens.len() {
                return false;
            }
            // any single token matches, continue
            continue;
        }

        if i >= command_tokens.len() {
            return false;
        }

        if *pat != command_tokens[i] {
            return false;
        }
    }

    // pattern fully consumed — command must be exactly the same length
    command_tokens.len() == pattern_tokens.len()
}

/// generates an "allow always" pattern from a command:
/// first token (executable) + `*`
pub fn generate_pattern(command: &str) -> String {
    let first = command.split_whitespace().next().unwrap_or(command);
    format!("{first} *")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_save_and_list_approval_rules() {
        let db = Database::open_in_memory().unwrap();
        db.save_approval_rule("ls *").unwrap();
        db.save_approval_rule("cargo *").unwrap();

        let rules = db.list_approval_rules().unwrap();
        assert_eq!(rules.len(), 2);
        assert_eq!(rules[0].pattern, "ls *");
        assert_eq!(rules[1].pattern, "cargo *");
    }

    #[test]
    fn test_save_approval_rule_ignores_duplicate() {
        let db = Database::open_in_memory().unwrap();
        db.save_approval_rule("ls *").unwrap();
        db.save_approval_rule("ls *").unwrap();

        let rules = db.list_approval_rules().unwrap();
        assert_eq!(rules.len(), 1);
    }

    #[test]
    fn test_delete_approval_rule() {
        let db = Database::open_in_memory().unwrap();
        db.save_approval_rule("ls *").unwrap();

        let rules = db.list_approval_rules().unwrap();
        assert!(db.delete_approval_rule(rules[0].id).unwrap());
        assert_eq!(db.list_approval_rules().unwrap().len(), 0);
    }

    #[test]
    fn test_find_matching_rule() {
        let db = Database::open_in_memory().unwrap();
        db.save_approval_rule("ls *").unwrap();

        assert!(db.find_matching_rule("ls -la").unwrap().is_some());
        assert!(db.find_matching_rule("ls").unwrap().is_some());
        assert!(db.find_matching_rule("rm -rf /").unwrap().is_none());
    }

    #[test]
    fn test_matches_rule_trailing_wildcard() {
        assert!(matches_rule("ls *", "ls"));
        assert!(matches_rule("ls *", "ls -la"));
        assert!(matches_rule("ls *", "ls -la /tmp"));
        assert!(!matches_rule("ls *", "rm foo"));
    }

    #[test]
    fn test_matches_rule_exact() {
        assert!(matches_rule("git status", "git status"));
        assert!(!matches_rule("git status", "git status -v"));
        assert!(!matches_rule("git status", "git"));
    }

    #[test]
    fn test_matches_rule_cargo_test() {
        assert!(matches_rule("cargo test *", "cargo test"));
        assert!(matches_rule("cargo test *", "cargo test -- --nocapture"));
    }

    #[test]
    fn test_matches_rule_pipe() {
        // both sub-commands must match
        assert!(matches_rule("ls *", "ls -la | ls /tmp"));
        assert!(!matches_rule("ls *", "ls -la | rm foo"));
    }

    #[test]
    fn test_matches_rule_chain() {
        assert!(matches_rule("cargo *", "cargo fmt && cargo test"));
        assert!(!matches_rule("cargo *", "cargo fmt && rm foo"));
    }

    #[test]
    fn test_generate_pattern() {
        assert_eq!(generate_pattern("ls -la /tmp"), "ls *");
        assert_eq!(generate_pattern("cargo test -- --nocapture"), "cargo *");
    }
}
