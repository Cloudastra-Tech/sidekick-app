//! Custom vocabulary for speech-to-text: names Deepgram would otherwise mishear (our company, our
//! clients, our services). Admins keep the list in a "Vocabulary" section of the playbook as a
//! bulleted list (one term per bullet, or comma-separated), and every call sends it to Deepgram
//! as key terms. Other lines in the section are notes and are ignored.

/// Most terms sent. Deepgram's key-term prompting is meant for a focused list.
pub const MAX_TERMS: usize = 50;

/// The terms in the playbook's "Vocabulary" section (any heading level), in order, without
/// duplicates (ignoring case), at most `MAX_TERMS`. No section, no terms.
pub fn from_playbook(playbook: &str) -> Vec<String> {
    let mut terms = crate::playbook::section_items(playbook, "vocabulary");
    terms.truncate(MAX_TERMS);
    terms
}

/// `&keyterm=…` for each term, percent-encoded for the query string.
pub fn query(terms: &[String]) -> String {
    terms
        .iter()
        .map(|term| format!("&keyterm={}", percent_encode(term)))
        .collect()
}

/// Everything but unreserved URL characters becomes `%XX` (UTF-8 bytes).
fn percent_encode(text: &str) -> String {
    text.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLAYBOOK: &str = "# Playbook\n\n## 1. Who we are\nAcme builds things.\n\n\
        ## Vocabulary\n\nNames for speech recognition, not facts for hints.\n\n\
        - Acme\n- NASSCOM, ICSI\n* Hugging Face\n- acme\n\n\
        ## 2. What we sell\n- Not a term\n";

    #[test]
    fn reads_the_vocabulary_section_only() {
        assert_eq!(
            from_playbook(PLAYBOOK),
            vec!["Acme", "NASSCOM", "ICSI", "Hugging Face"]
        );
    }

    #[test]
    fn a_numbered_heading_counts() {
        assert_eq!(
            from_playbook("## 8. Vocabulary\n- Acme\n## 9. Other\n- No\n"),
            vec!["Acme"]
        );
    }

    #[test]
    fn no_section_means_no_terms() {
        assert!(from_playbook("# Playbook\n\n## Objections\n- Too expensive\n").is_empty());
    }

    #[test]
    fn the_list_is_capped() {
        let many: String = (0..80).map(|i| format!("- Term{i}\n")).collect();
        let terms = from_playbook(&format!("### Vocabulary\n{many}"));
        assert_eq!(terms.len(), MAX_TERMS);
        assert_eq!(terms[0], "Term0");
    }

    #[test]
    fn terms_are_encoded_for_the_url() {
        let terms = vec![
            "Hugging Face".to_string(),
            "R&D".to_string(),
            "Noida".to_string(),
        ];
        assert_eq!(
            query(&terms),
            "&keyterm=Hugging%20Face&keyterm=R%26D&keyterm=Noida"
        );
        assert_eq!(
            query(&["कितना".to_string()]),
            "&keyterm=%E0%A4%95%E0%A4%BF%E0%A4%A4%E0%A4%A8%E0%A4%BE"
        );
        assert_eq!(query(&[]), "");
    }

    #[test]
    fn the_shipped_playbook_has_a_vocabulary() {
        let terms = from_playbook(include_str!("../../playbook/sales.md"));
        for name in ["Acme Cloud", "Northwind Learning", "Kubernetes", "FastAPI"] {
            assert!(terms.iter().any(|t| t == name), "{name} missing: {terms:?}");
        }
        assert!(terms.len() <= MAX_TERMS);
    }
}
