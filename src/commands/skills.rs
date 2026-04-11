use crate::skill;

pub(crate) fn run_skills() {
    let skills = skill::load_skills();

    if skills.is_empty() {
        println!("no skills installed");
        println!("add skills to ~/.ava/skills/<name>/SKILL.md");
        println!("(skills from ~/.claude/skills/ are also loaded)");
        return;
    }

    println!("{:<20} {:<6} DESCRIPTION", "NAME", "USER");
    for s in &skills {
        let user = if s.user_invocable { "yes" } else { "-" };
        println!("{:<20} {:<6} {}", s.name, user, s.description);
    }
}
