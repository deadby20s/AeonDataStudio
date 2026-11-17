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

/// (topic, subjects, objects). Sentences are subject + verb + object + adverbial, each slot
/// used at most once per article, so clean articles never repeat phrases (which the
/// repetition filters would rightly flag).
const TOPICS: [(&str, [&str; 12], [&str; 12]); 5] = [
    (
        "science",
        ["The research team", "A young biologist", "Scientists at the institute", "The laboratory director", "A group of students", "The survey crew", "Two visiting physicists", "The field station", "Our science editor", "The chemistry department", "An astronomer in Chile", "The ocean project"],
        ["the ocean temperature records", "a rare species of beetle", "the new battery chemistry", "samples from the glacier", "the migration of swallows", "data from the telescope", "the soil under the old forest", "a faster way to sequence DNA", "the effects of a dry summer", "the light from distant galaxies", "pollen in the lake sediment", "the strength of spider silk"],
    ),
    (
        "cooking",
        ["The chef", "My grandmother", "The bakery on the corner", "A food writer", "Our cooking club", "The village restaurant", "A pastry student", "The market stall owner", "The school canteen", "A home cook in Lyon", "The test kitchen", "Our neighbour"],
        ["a slow-roasted tomato sauce", "the sourdough starter", "a spiced pumpkin soup", "fresh pasta with wild garlic", "the lemon tart recipe", "a pot of mushroom risotto", "the summer fruit preserves", "a simple flatbread", "the family stew", "a plate of grilled vegetables", "the apple cake", "a jar of chilli oil"],
    ),
    (
        "travel",
        ["A walking guide", "The ferry company", "Our travel writer", "A family from Oslo", "The tourist office", "Two cyclists", "The mountain hut keeper", "A photographer", "The harbour master", "The local historian", "A group of hikers", "The railway museum"],
        ["the coastal footpath", "the old harbour district", "a quiet valley village", "the night train timetable", "the castle ruins", "the island ferry route", "a market in the square", "the lighthouse trail", "the mountain pass", "the river cruise", "a small mountain lake", "the cathedral tower"],
    ),
    (
        "history",
        ["The archive", "A local historian", "The museum curator", "Letters from the period", "The town records", "A retired teacher", "The parish register", "An old newspaper", "The excavation team", "A family diary", "The city council", "The library collection"],
        ["the first stone bridge", "the founding of the school", "trade along the old canal", "the great flood", "the abandoned castle", "life in the busy port", "the wartime factory", "the medieval market rules", "the railway strike", "the town's oldest mill", "the lost village", "the earliest street map"],
    ),
    (
        "technology",
        ["The open-source project", "Our engineering team", "The new compiler", "A security researcher", "The database vendor", "Two maintainers", "The browser team", "A student developer", "The release manager", "The cloud provider", "A hardware startup", "The accessibility group"],
        ["the memory allocator", "the file import code", "a rare crash at startup", "the search index", "keyboard navigation", "the build system", "support for compressed archives", "the error messages", "the plugin interface", "battery use on laptops", "the network protocol", "the documentation site"],
    ),
];

const VERBS: [&str; 12] = [
    "studied", "described", "improved", "tested", "explained", "measured", "compared", "reviewed", "documented",
    "examined", "rebuilt", "photographed",
];

const ADVERBIALS: [&str; 12] = [
    "during the spring", "over the past decade", "in a small village near the coast", "with help from local volunteers",
    "after several months of careful work", "for the first time last year", "despite a difficult start",
    "in a report published this week", "with surprisingly good results", "before the winter arrived",
    "as part of a larger project", "under the guidance of an experienced mentor",
];

const CONNECTORS: [&str; 6] = ["In addition,", "However,", "As a result,", "Meanwhile,", "For example,", "Over time,"];

/// `count` distinct indices below `n`, in random order.
fn distinct(rng: &mut Rng, n: usize, count: usize) -> Vec<usize> {
    let mut idx: Vec<usize> = (0..n).collect();
    for i in 0..count.min(n) {
        let j = i + rng.below(n - i);
        idx.swap(i, j);
    }
    idx.truncate(count.min(n));
    idx
}

fn compose(subject: &str, verb: &str, object: &str, adverbial: &str, connector: Option<&str>) -> String {
    let s = format!("{subject} {verb} {object} {adverbial}.");
    match connector {
        Some(c) => {
            let mut chars = s.chars();
            let first = chars.next().map(|c| c.to_lowercase().to_string()).unwrap_or_default();
            format!("{c} {first}{}", chars.as_str())
        }
        None => s,
    }
}

/// One random sentence on `topic`.
fn sentence(rng: &mut Rng, topic: usize) -> String {
    let (_, subjects, objects) = &TOPICS[topic];
    let (s, v, o, a) = (rng.below(12), rng.below(12), rng.below(12), rng.below(12));
    compose(subjects[s], VERBS[v], objects[o], ADVERBIALS[a], None)
}

/// An article of 6–12 sentences in 2–3 paragraphs, never repeating a phrase.
fn article(rng: &mut Rng, topic: usize) -> String {
    let (_, subjects, objects) = &TOPICS[topic];
    let n = 6 + rng.below(7);
    let (s, v, o, a) = (distinct(rng, 12, n), distinct(rng, 12, n), distinct(rng, 12, n), distinct(rng, 12, n));
    let connectors = distinct(rng, CONNECTORS.len(), CONNECTORS.len());
    let mut used_connectors = 0;
    let sentences: Vec<String> = (0..n)
        .map(|i| {
            let connector = (i > 0 && used_connectors < connectors.len() && rng.below(4) == 0).then(|| {
                used_connectors += 1;
                CONNECTORS[connectors[used_connectors - 1]]
            });
            compose(subjects[s[i]], VERBS[v[i]], objects[o[i]], ADVERBIALS[a[i]], connector)
        })
        .collect();
    let paragraphs = 2 + rng.below(2);
    let per = n.div_ceil(paragraphs);
    sentences.chunks(per).map(|c| c.join(" ")).collect::<Vec<_>>().join("\n\n")
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

    /// Clean articles must survive the default template (without dedup, which needs the whole
    /// corpus) and most planted problems must be caught, or the demo would mislead testers.
    #[test]
    fn template_keeps_clean_articles_and_catches_planted_problems() {
        use crate::pipeline::{Decisions, Pipeline};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("demo.jsonl");
        write_jsonl(&path).unwrap();
        let mut spec = aeon_shared::PipelineSpec::template("t");
        spec.steps.retain(|s| !s.config.kind().ends_with("_dedup"));
        let p = Pipeline::compile(&spec).unwrap();
        let mut by_cat: std::collections::BTreeMap<String, (u32, u32)> = Default::default();
        for line in std::fs::read_to_string(&path).unwrap().lines() {
            let v: serde_json::Value = serde_json::from_str(line).unwrap();
            let cat = v["category"].as_str().unwrap().to_string();
            let kept = p.process(v["text"].as_str().unwrap().to_string(), 0, &Decisions::default(), None).dropped.is_none();
            let e = by_cat.entry(cat).or_default();
            e.0 += 1;
            e.1 += kept as u32;
        }
        let share = |c: &str| {
            let (n, kept) = by_cat[c];
            kept as f64 / n as f64
        };
        assert!(share("clean") > 0.95, "clean kept: {by_cat:?}");
        for planted in ["german", "french", "spanish", "menu", "spam", "too_short", "numbers", "all_caps", "lorem_ipsum"] {
            assert!(share(planted) < 0.1, "{planted} mostly kept: {by_cat:?}");
        }
        // Fixable problems are fixed, not removed.
        for fixable in ["mojibake", "messy_whitespace"] {
            assert!(share(fixable) > 0.9, "{fixable} removed: {by_cat:?}");
        }
    }
}
