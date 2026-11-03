//! Lists kept in the playbook as their own sections, so admins edit them where they edit the
//! rest of the coaching context: the speech-to-text vocabulary, the trigger words.

/// The bullet items of the playbook section titled `title` (any heading level, an optional
/// "8." style number, case-insensitive), in order, comma-separated items split, duplicates
/// (ignoring case) dropped. Lines that aren't bullets are notes and are skipped. No section,
/// no items.
pub fn section_items(playbook: &str, title: &str) -> Vec<String> {
    let mut items: Vec<String> = Vec::new();
    let mut in_section = false;
    for line in playbook.lines() {
        let line = line.trim();
        if let Some(heading) = line.strip_prefix('#') {
            in_section = heading
                .trim_start_matches('#')
                .trim()
                .trim_start_matches(|c: char| c.is_ascii_digit() || c == '.')
                .trim()
                .eq_ignore_ascii_case(title);
            continue;
        }
        if !in_section {
            continue;
        }
        let Some(item) = line.strip_prefix("- ").or_else(|| line.strip_prefix("* ")) else {
            continue; // a note about the list
        };
        for entry in item.split(',').map(str::trim).filter(|e| !e.is_empty()) {
            if !items
                .iter()
                .any(|i| i.to_lowercase() == entry.to_lowercase())
            {
                items.push(entry.to_string());
            }
        }
    }
    items
}
