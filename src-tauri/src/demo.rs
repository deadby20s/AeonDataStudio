//! A generated demo dataset, so testers can try every step without their own data.
//!
//! About 3,000 web-like documents: mostly clean prose, plus planted examples of everything
//! the template pipeline catches. Each document's `meta.category` says what it is, so the
//! Inspector's results can be checked at a glance. Generation is deterministic.

use std::io::{BufWriter, Write};
use std::path::Path;

use crate::error::AppResult;

pub const DATASET_NAME: &str = "Demo: mixed web text";

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
    fn pick<'a>(&mut self, items: &[&'a str]) -> &'a str {
        items[self.below(items.len())]
    }
}

const TOPICS: [(&str, &[&str], &[&str]); 5] = [
    (
        "science",
        &["The research team", "A new study", "Scientists at the institute", "The laboratory", "An international group of researchers"],
        &["measured changes in ocean temperature over two decades", "found that the protein folds in an unexpected way", "published data from the latest telescope survey", "tested the new battery chemistry under extreme cold", "tracked the migration of birds across three continents"],
    ),
    (
        "cooking",
        &["The chef", "Our kitchen", "Home cooks", "The bakery on the corner", "A family recipe"],
        &["roasts the vegetables slowly to bring out their sweetness", "lets the dough rest overnight before shaping it", "balances the sauce with a little lemon juice", "uses seasonal produce from nearby farms", "serves the soup with fresh bread and herbs"],
    ),
    (
        "travel",
        &["The old town", "Visitors to the coast", "The mountain railway", "Our guide", "The harbour district"],
        &["is best explored on foot early in the morning", "offers views across the valley from the upper station", "fills with markets and music during the summer festival", "recommends booking the ferry a few days ahead", "has narrow streets lined with small cafés"],
    ),
    (
        "history",
        &["The archive", "Historians", "The museum collection", "Letters from the period", "The town records"],
        &["show how trade routes shifted after the war", "describe the construction of the first stone bridge", "reveal daily life in a busy port city", "document the founding of the local school", "explain why the castle was abandoned"],
    ),
    (
        "technology",
        &["The update", "Engineers", "The open-source project", "The new compiler", "Our team"],
        &["reduces memory use when processing large files", "adds support for reading compressed archives", "fixes a rare crash during startup", "makes the search feature noticeably faster", "improves accessibility for keyboard users"],
    ),
];

const CONNECTORS: [&str; 6] = ["In addition,", "However,", "As a result,", "Meanwhile,", "For example,", "Over time,"];

fn sentence(rng: &mut Rng, topic: usize) -> String {
    let (_, subjects, predicates) = TOPICS[topic];
    let s = format!("{} {}.", rng.pick(subjects), rng.pick(predicates));
    if rng.below(3) == 0 {
        let c = rng.pick(&CONNECTORS);
        let mut chars = s.chars();
        let first = chars.next().map(|c| c.to_lowercase().to_string()).unwrap_or_default();
        format!("{c} {first}{}", chars.as_str())
    } else {
        s
    }
}

fn article(rng: &mut Rng, topic: usize) -> String {
    let paragraphs = 2 + rng.below(4);
    (0..paragraphs)
        .map(|_| (0..3 + rng.below(4)).map(|_| sentence(rng, topic)).collect::<Vec<_>>().join(" "))
        .collect::<Vec<_>>()
        .join("\n\n")
}

const GERMAN: [&str; 5] = [
    "Die Forscher haben neue Daten über die Temperatur der Ozeane veröffentlicht.",
    "Der Koch lässt den Teig über Nacht ruhen, bevor er ihn formt.",
    "Die Altstadt erkundet man am besten früh am Morgen zu Fuß.",
    "Das Archiv zeigt, wie sich die Handelswege nach dem Krieg verändert haben.",
    "Das Update reduziert den Speicherverbrauch bei großen Dateien deutlich.",
];
const FRENCH: [&str; 5] = [
    "Les chercheurs ont publié de nouvelles données sur la température des océans.",
    "Le chef laisse reposer la pâte toute la nuit avant de la façonner.",
    "La vieille ville se découvre mieux à pied, tôt le matin.",
    "Les archives montrent comment les routes commerciales ont changé après la guerre.",
    "La mise à jour réduit fortement la mémoire utilisée pour les gros fichiers.",
];
const SPANISH: [&str; 5] = [
    "Los investigadores publicaron nuevos datos sobre la temperatura de los océanos.",
    "El cocinero deja reposar la masa toda la noche antes de darle forma.",
    "El casco antiguo se recorre mejor a pie, temprano por la mañana.",
    "Los archivos muestran cómo cambiaron las rutas comerciales después de la guerra.",
    "La actualización reduce mucho la memoria usada con archivos grandes.",
];

/// UTF-8 text decoded as Windows-1252, the most common form of mojibake.
fn mojibake(s: &str) -> String {
    let (decoded, _, _) = encoding_rs::WINDOWS_1252.decode(s.as_bytes());
    decoded.into_owned()
}

/// Writes the demo dataset as JSONL. Returns the number of documents.
pub fn write_jsonl(path: &Path) -> AppResult<u64> {
    let mut rng = Rng(20_261_115);
    let mut out = BufWriter::new(std::fs::File::create(path)?);
    let mut n = 0u64;
    let mut clean: Vec<String> = Vec::new();
    let write = |out: &mut BufWriter<std::fs::File>, text: &str, category: &str, n: &mut u64| -> AppResult<()> {
        let line = serde_json::json!({
            "id": format!("demo-{:05}", *n),
            "text": text,
            "url": format!("https://example.com/{category}/{}", *n),
            "category": category,
        });
        writeln!(out, "{line}")?;
        *n += 1;
        Ok(())
    };

    for _ in 0..3000usize {
        let roll = rng.below(100);
        let topic = rng.below(TOPICS.len());
        let (text, category): (String, &str) = match roll {
            // Most documents are clean articles.
            0..=59 => {
                let a = article(&mut rng, topic);
                clean.push(a.clone());
                (a, "clean")
            }
            // Exact copies, some differing only in case or spacing.
            60..=64 if !clean.is_empty() => {
                let src = clean[rng.below(clean.len())].clone();
                if rng.below(2) == 0 { (src, "exact_duplicate") } else { (src.replace(". ", ".  "), "exact_duplicate_spacing") }
            }
            // Near copies: one sentence replaced, or a footer added.
            65..=69 if !clean.is_empty() => {
                let src = clean[rng.below(clean.len())].clone();
                if rng.below(2) == 0 {
                    (format!("{src}\n\nPosted on 2026-{:02}-{:02}. Share this article.", 1 + rng.below(12), 1 + rng.below(28)), "near_duplicate_footer")
                } else {
                    let mut sentences: Vec<String> = src.split(". ").map(String::from).collect();
                    let k = rng.below(sentences.len());
                    sentences[k] = sentence(&mut rng, topic).trim_end_matches('.').to_string();
                    (sentences.join(". "), "near_duplicate_edit")
                }
            }
            // Other languages.
            70..=74 => {
                let (pool, cat) = match rng.below(3) {
                    0 => (&GERMAN, "german"),
                    1 => (&FRENCH, "french"),
                    _ => (&SPANISH, "spanish"),
                };
                let t = (0..12).map(|_| rng.pick(pool)).collect::<Vec<_>>().join(" ");
                (t, cat)
            }
            // Navigation menus and link lists.
            75..=78 => {
                let items = ["Home", "About us", "Products", "Blog", "Careers", "Contact", "Privacy policy", "Terms", "Login", "Sign up", "FAQ", "Support"];
                let t = (0..40 + rng.below(30)).map(|_| format!("• {}", rng.pick(&items))).collect::<Vec<_>>().join("\n");
                (t, "menu")
            }
            // Repetitive spam.
            79..=82 => {
                let phrase = rng.pick(&["best cheap shoes buy now", "click here for free prizes", "top deals today only"]);
                (format!("{phrase} ").repeat(25 + rng.below(20)), "spam")
            }
            // Too short.
            83..=86 => (rng.pick(&["Click here to continue.", "Page not found.", "Loading…", "Subscribe to our newsletter!", "404"]).to_string(), "too_short"),
            // Mostly numbers (tables, logs).
            87..=89 => {
                let t = (0..80).map(|_| format!("{}.{:02} {} {}", rng.below(1000), rng.below(100), rng.below(10_000), rng.below(99))).collect::<Vec<_>>().join("\n");
                (t, "numbers")
            }
            // Shouting.
            90..=91 => (article(&mut rng, topic).to_uppercase(), "all_caps"),
            // Placeholder text.
            92..=93 => (format!("Lorem ipsum dolor sit amet, consectetur adipiscing elit. {}", article(&mut rng, topic)), "lorem_ipsum"),
            // Broken encoding (fixable).
            94..=96 => (mojibake(&format!("The café’s naïve résumé — {}", article(&mut rng, topic))), "mojibake"),
            // Messy whitespace and invisible characters (fixable).
            _ => {
                let a = article(&mut rng, topic);
                (format!("  {}\u{200B}\r\n\r\n\r\n\r\n", a.replace(". ", ".   ").replace("\n\n", "\n\n\n\n\u{00A0}")), "messy_whitespace")
            }
        };
        write(&mut out, &text, category, &mut n)?;
    }
    out.flush()?;
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_every_category_deterministically() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (dir.path().join("a.jsonl"), dir.path().join("b.jsonl"));
        assert_eq!(write_jsonl(&a).unwrap(), 3000);
        write_jsonl(&b).unwrap();
        let text = std::fs::read_to_string(&a).unwrap();
        assert_eq!(text, std::fs::read_to_string(&b).unwrap(), "same seed, same file");
        for cat in ["clean", "exact_duplicate", "near_duplicate_footer", "german", "menu", "spam", "too_short", "numbers", "all_caps", "lorem_ipsum", "mojibake", "messy_whitespace"] {
            assert!(text.contains(&format!("\"category\":\"{cat}\"")), "missing {cat}");
        }
        assert!(text.contains("cafÃ©"), "mojibake is really broken");
    }
}
