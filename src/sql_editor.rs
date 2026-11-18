//! The SQL editor: syntax highlighting, live checking and snippets.
//!
//! Highlighting is a `<pre>` drawn behind a transparent `<textarea>` with the same font and
//! padding, so typing, selection and undo stay the browser's own.

use aeon_shared::{SqlCheck, SqlTable};
use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::api;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tok {
    Keyword,
    Function,
    Type,
    Str,
    Number,
    Comment,
    Table,
    Ident,
    Operator,
    Space,
}

impl Tok {
    fn class(self) -> &'static str {
        match self {
            Tok::Keyword => "tk-kw",
            Tok::Function => "tk-fn",
            Tok::Type => "tk-ty",
            Tok::Str => "tk-str",
            Tok::Number => "tk-num",
            Tok::Comment => "tk-com",
            Tok::Table => "tk-tbl",
            Tok::Ident => "tk-id",
            Tok::Operator => "tk-op",
            Tok::Space => "",
        }
    }
}

const KEYWORDS: &[&str] = &[
    "select", "from", "where", "group", "by", "order", "having", "limit", "offset", "as", "and", "or", "not", "in",
    "is", "null", "like", "ilike", "between", "case", "when", "then", "else", "end", "join", "left", "right", "inner",
    "outer", "full", "cross", "on", "using", "union", "all", "distinct", "with", "asc", "desc", "nulls", "first",
    "last", "insert", "into", "values", "update", "set", "delete", "create", "table", "view", "drop", "alter", "copy",
    "to", "format", "true", "false", "exists", "any", "some", "filter", "over", "partition", "window", "qualify",
    "sample", "using", "percent", "rows", "range", "unbounded", "preceding", "following", "current", "row", "describe",
    "summarize", "explain", "analyze", "pivot", "unpivot", "except", "intersect", "lateral", "natural", "semi",
    "anti", "asof", "recursive", "replace", "exclude", "columns", "if", "temp", "temporary", "or", "glob",
    "similar", "escape", "collate", "interval", "struct", "list", "map",
];

const TYPES: &[&str] = &[
    "varchar", "text", "integer", "int", "bigint", "smallint", "tinyint", "hugeint", "double", "float", "real",
    "decimal", "numeric", "boolean", "bool", "date", "time", "timestamp", "blob", "json", "uuid", "ubigint",
    "uinteger",
];

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Splits SQL into highlighted pieces. Unknown or unfinished input still produces tokens.
pub fn tokenize(sql: &str, tables: &[String]) -> Vec<(Tok, String)> {
    let chars: Vec<char> = sql.chars().collect();
    let mut out: Vec<(Tok, String)> = Vec::new();
    let mut i = 0;
    let take = |from: usize, to: usize| chars[from..to].iter().collect::<String>();
    while i < chars.len() {
        let c = chars[i];
        let start = i;
        let tok = if c.is_whitespace() {
            while i < chars.len() && chars[i].is_whitespace() {
                i += 1;
            }
            Tok::Space
        } else if c == '-' && chars.get(i + 1) == Some(&'-') {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
            Tok::Comment
        } else if c == '/' && chars.get(i + 1) == Some(&'*') {
            i += 2;
            while i < chars.len() && !(chars[i] == '*' && chars.get(i + 1) == Some(&'/')) {
                i += 1;
            }
            i = (i + 2).min(chars.len());
            Tok::Comment
        } else if c == '\'' {
            i += 1;
            while i < chars.len() {
                if chars[i] == '\'' {
                    // '' is an escaped quote.
                    if chars.get(i + 1) == Some(&'\'') {
                        i += 2;
                        continue;
                    }
                    i += 1;
                    break;
                }
                i += 1;
            }
            Tok::Str
        } else if c == '"' {
            i += 1;
            while i < chars.len() && chars[i] != '"' {
                i += 1;
            }
            i = (i + 1).min(chars.len());
            Tok::Ident
        } else if c.is_ascii_digit() || (c == '.' && chars.get(i + 1).is_some_and(char::is_ascii_digit)) {
            while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '.' || chars[i] == '_') {
                i += 1;
            }
            Tok::Number
        } else if is_word_char(c) {
            while i < chars.len() && is_word_char(chars[i]) {
                i += 1;
            }
            let word = take(start, i).to_lowercase();
            let mut j = i;
            while j < chars.len() && chars[j] == ' ' {
                j += 1;
            }
            if chars.get(j) == Some(&'(') && !KEYWORDS.contains(&word.as_str()) {
                Tok::Function
            } else if tables.iter().any(|t| t.eq_ignore_ascii_case(&word)) {
                Tok::Table
            } else if TYPES.contains(&word.as_str()) {
                Tok::Type
            } else if KEYWORDS.contains(&word.as_str()) {
                Tok::Keyword
            } else {
                Tok::Ident
            }
        } else {
            i += 1;
            // Two-character operators stay together: ->> -> :: <= >= <> != ||
            while i < chars.len() && "-<>=!|:".contains(chars[i]) && "-<>=!|:".contains(chars[i - 1]) && i - start < 3 {
                i += 1;
            }
            Tok::Operator
        };
        out.push((tok, take(start, i)));
    }
    out
}

/// Character offset of (1-based line, 0-based column).
fn offset_of(sql: &str, line: u32, column: u32) -> usize {
    let mut offset = 0;
    for (n, l) in sql.split('\n').enumerate() {
        if n + 1 == line as usize {
            return offset + (column as usize).min(l.chars().count());
        }
        offset += l.chars().count() + 1;
    }
    offset
}

/// Ready-made queries. `{t}` is a dataset table, `{r}` a removed-documents table.
pub struct Snippet {
    pub group: &'static str,
    pub label: &'static str,
    pub sql: &'static str,
}

pub const SNIPPETS: &[Snippet] = &[
    Snippet { group: "Look", label: "Preview documents", sql: "SELECT id, source, length(text) AS chars, left(text, 200) AS start\nFROM {t}\nLIMIT 100;" },
    Snippet { group: "Look", label: "Random sample", sql: "SELECT id, left(text, 300) AS start\nFROM {t}\nUSING SAMPLE 20 ROWS;" },
    Snippet { group: "Look", label: "Search text", sql: "SELECT id, left(text, 200) AS start\nFROM {t}\nWHERE text ILIKE '%keyword%'\nLIMIT 100;" },
    Snippet { group: "Look", label: "Regex search", sql: "SELECT id, regexp_extract(text, '(https?://\\S+)') AS first_url\nFROM {t}\nWHERE regexp_matches(text, 'https?://')\nLIMIT 100;" },
    Snippet { group: "Look", label: "Shortest documents", sql: "SELECT id, length(text) AS chars, text\nFROM {t}\nORDER BY chars\nLIMIT 50;" },
    Snippet { group: "Stats", label: "Length statistics", sql: "SELECT count(*) AS docs,\n       avg(length(text))::INT AS mean_chars,\n       median(length(text)) AS median_chars,\n       quantile_cont(length(text), 0.95)::INT AS p95_chars,\n       max(length(text)) AS max_chars\nFROM {t};" },
    Snippet { group: "Stats", label: "Length histogram", sql: "SELECT (length(text) // 500) * 500 AS chars_from, count(*) AS docs\nFROM {t}\nGROUP BY 1\nORDER BY 1;" },
    Snippet { group: "Stats", label: "Words per document", sql: "SELECT avg(len(string_split_regex(trim(text), '\\s+')))::INT AS mean_words,\n       median(len(string_split_regex(trim(text), '\\s+'))) AS median_words\nFROM {t};" },
    Snippet { group: "Stats", label: "Documents per source", sql: "SELECT source, count(*) AS docs, sum(length(text)) AS chars\nFROM {t}\nGROUP BY source\nORDER BY docs DESC;" },
    Snippet { group: "Stats", label: "Metadata field values", sql: "SELECT meta->>'lang' AS value, count(*) AS docs\nFROM {t}\nGROUP BY 1\nORDER BY docs DESC;" },
    Snippet { group: "Stats", label: "Top web domains", sql: "SELECT regexp_extract(meta->>'url', '://([^/]+)', 1) AS domain, count(*) AS docs\nFROM {t}\nGROUP BY 1\nORDER BY docs DESC\nLIMIT 50;" },
    Snippet { group: "Quality", label: "Exact duplicate texts", sql: "SELECT md5(text) AS hash, count(*) AS copies, any_value(left(text, 120)) AS start\nFROM {t}\nGROUP BY hash\nHAVING copies > 1\nORDER BY copies DESC\nLIMIT 100;" },
    Snippet { group: "Quality", label: "Repeated first lines", sql: "SELECT split_part(text, chr(10), 1) AS first_line, count(*) AS docs\nFROM {t}\nGROUP BY 1\nHAVING docs > 5\nORDER BY docs DESC\nLIMIT 50;" },
    Snippet { group: "Quality", label: "Mostly non-letters", sql: "SELECT id, left(text, 200) AS start\nFROM {t}\nWHERE length(regexp_replace(text, '[^[:alpha:]]', '', 'g')) < 0.5 * length(text)\nLIMIT 100;" },
    Snippet { group: "Quality", label: "Score histogram", sql: "-- Scores saved by an LLM score step in \"Keep all, save the score\" mode
SELECT round((meta->>'llm_score')::DOUBLE) AS score, count(*) AS docs
FROM {t}
WHERE meta->>'llm_score' IS NOT NULL
GROUP BY 1
ORDER BY 1;" },
    Snippet { group: "Quality", label: "Leftover HTML", sql: "SELECT id, left(text, 200) AS start\nFROM {t}\nWHERE regexp_matches(text, '</?(div|span|a|p|br)[ >]')\nLIMIT 100;" },
    Snippet { group: "Runs", label: "Removed by step and rule", sql: "SELECT step_index, rule, count(*) AS docs\nFROM {r}\nGROUP BY ALL\nORDER BY docs DESC;" },
    Snippet { group: "Runs", label: "Removed examples for a rule", sql: "SELECT detail, left(text, 200) AS start\nFROM {r}\nWHERE rule = 'min_words'\nLIMIT 50;" },
    Snippet { group: "Runs", label: "Removed share per source", sql: "SELECT source, count(*) AS removed\nFROM {r}\nGROUP BY source\nORDER BY removed DESC;" },
    Snippet { group: "Export", label: "Save results to Parquet", sql: "COPY (\n  SELECT id, text\n  FROM {t}\n  WHERE length(text) > 1000\n) TO 'C:/path/to/result.parquet' (FORMAT parquet);" },
];

/// The snippet text for the tables at hand, or `None` if it needs a table that doesn't exist.
pub fn fill(snippet: &Snippet, dataset: Option<&str>, removed: Option<&str>) -> Option<String> {
    let mut s = snippet.sql.to_string();
    if s.contains("{t}") {
        s = s.replace("{t}", dataset?);
    }
    if s.contains("{r}") {
        s = s.replace("{r}", removed?);
    }
    Some(s)
}

#[component]
pub fn SqlEditor(
    sql: RwSignal<String>,
    tables: Signal<Vec<SqlTable>>,
    on_run: impl Fn() + Copy + Send + Sync + 'static,
) -> impl IntoView {
    let check = RwSignal::new(SqlCheck::default());
    let checking = RwSignal::new(false);
    let textarea: NodeRef<leptos::html::Textarea> = NodeRef::new();
    let backdrop: NodeRef<leptos::html::Pre> = NodeRef::new();
    let show_snippets = RwSignal::new(false);
    // Only the newest check's answer is shown.
    let generation = StoredValue::new(0u64);

    // Check a moment after typing stops.
    Effect::new(move |_| {
        let q = sql.get();
        generation.update_value(|g| *g += 1);
        let mine = generation.get_value();
        checking.set(true);
        spawn_local(async move {
            gloo_sleep(350).await;
            if generation.get_value() != mine {
                return;
            }
            let result = api::check_sql(&q).await;
            if generation.get_value() != mine {
                return;
            }
            check.set(result.unwrap_or_else(|e| SqlCheck { error: Some(e.to_string()), line: None, column: None }));
            checking.set(false);
        });
    });

    let table_names = move || tables.with(|t| t.iter().map(|t| t.name.clone()).collect::<Vec<_>>());

    let highlighted = move || {
        let text = sql.get();
        let names = table_names();
        let c = check.get();
        let err_at = c.line.map(|l| offset_of(&text, l, c.column.unwrap_or(0)));
        let mut pos = 0usize;
        let mut views = Vec::new();
        for (tok, piece) in tokenize(&text, &names) {
            let len = piece.chars().count();
            let marked = err_at.is_some_and(|e| tok != Tok::Space && e >= pos && e < pos + len.max(1));
            let class = format!("{}{}", tok.class(), if marked { " tk-err" } else { "" });
            pos += len;
            views.push(view! { <span class=class>{piece}</span> });
        }
        // A trailing newline needs something after it, or the backdrop is a line short.
        views.push(view! { <span class=String::new()>{" ".to_string()}</span> });
        views
    };

    let sync_scroll = move || {
        if let (Some(t), Some(b)) = (textarea.get(), backdrop.get()) {
            b.set_scroll_top(t.scroll_top());
            b.set_scroll_left(t.scroll_left());
        }
    };

    let insert = move |text: String| {
        let current = sql.get_untracked();
        let (start, end) = textarea
            .get()
            .and_then(|t| Some((t.selection_start().ok()??, t.selection_end().ok()??)))
            .map(|(s, e)| (s as usize, e as usize))
            .unwrap_or((current.chars().count(), current.chars().count()));
        // Selection offsets are UTF-16 units; convert through chars for non-ASCII text.
        let utf16_to_byte = |u: usize| {
            let mut units = 0;
            for (b, ch) in current.char_indices() {
                if units >= u {
                    return b;
                }
                units += ch.len_utf16();
            }
            current.len()
        };
        let (bs, be) = (utf16_to_byte(start), utf16_to_byte(end));
        let mut new = String::new();
        new.push_str(&current[..bs]);
        new.push_str(&text);
        new.push_str(&current[be..]);
        let caret = (current[..bs].encode_utf16().count() + text.encode_utf16().count()) as u32;
        sql.set(new);
        if let Some(t) = textarea.get() {
            let _ = t.focus();
            request_animation_frame(move || {
                let _ = t.set_selection_range(caret, caret);
            });
        }
    };

    let first_of = move |kind: &str| tables.with(|t| t.iter().find(|t| t.kind == kind).map(|t| t.name.clone()));

    view! {
        <div class="sql-toolbar">
            <div class="snippet-menu">
                <button on:click=move |_| show_snippets.update(|v| *v = !*v) aria-haspopup="true" aria-expanded=move || show_snippets.get().to_string()>
                    "Snippets ▾"
                </button>
                <Show when=move || show_snippets.get()>
                    <div class="snippet-pop" role="menu">
                        {move || {
                            let dataset = first_of("dataset");
                            let removed = first_of("removed");
                            let mut groups: Vec<&str> = Vec::new();
                            for s in SNIPPETS {
                                if !groups.contains(&s.group) {
                                    groups.push(s.group);
                                }
                            }
                            groups
                                .into_iter()
                                .map(|g| {
                                    let items = SNIPPETS
                                        .iter()
                                        .filter(|s| s.group == g)
                                        .map(|s| {
                                            let filled = fill(s, dataset.as_deref(), removed.as_deref());
                                            let enabled = filled.is_some();
                                            let preview = filled.clone().unwrap_or_else(|| "Needs a pipeline run first".into());
                                            view! {
                                                <button
                                                    class="snippet-item"
                                                    role="menuitem"
                                                    title=preview
                                                    disabled=!enabled
                                                    on:click=move |_| {
                                                        if let Some(q) = filled.clone() {
                                                            insert(q);
                                                            show_snippets.set(false);
                                                        }
                                                    }
                                                >
                                                    {s.label}
                                                </button>
                                            }
                                        })
                                        .collect_view();
                                    view! {
                                        <div class="snippet-group">
                                            <div class="small-heading">{g}</div>
                                            {items}
                                        </div>
                                    }
                                })
                                .collect_view()
                        }}
                    </div>
                </Show>
            </div>
            <span class="sql-status" aria-live="polite">
                {move || {
                    let c = check.get();
                    match (&c.error, checking.get()) {
                        (_, true) if sql.with(|s| s.trim().is_empty()) => view! { <span></span> }.into_any(),
                        (None, true) => view! { <span class="muted small">"Checking…"</span> }.into_any(),
                        (None, false) if sql.with(|s| s.trim().is_empty()) => view! { <span></span> }.into_any(),
                        (None, false) => view! { <span class="ok small">"✓ Valid"</span> }.into_any(),
                        (Some(e), _) => {
                            let place = match (c.line, c.column) {
                                (Some(l), Some(col)) => format!("Line {l}, column {}: ", col + 1),
                                (Some(l), None) => format!("Line {l}: "),
                                _ => String::new(),
                            };
                            let first = e.lines().next().unwrap_or("").to_string();
                            view! { <span class="error small" title=e.clone()>"✗ " {place} {first}</span> }.into_any()
                        }
                    }
                }}
            </span>
        </div>
        <div class="sql-editor-wrap">
            <pre class="sql-editor sql-backdrop" aria-hidden="true" node_ref=backdrop>
                {highlighted}
            </pre>
            <textarea
                class="sql-editor sql-input"
                aria-label="SQL query"
                spellcheck="false"
                autocomplete="off"
                node_ref=textarea
                prop:value=move || sql.get()
                on:input=move |ev| sql.set(event_target_value(&ev))
                on:scroll=move |_| sync_scroll()
                on:keydown=move |ev| {
                    if ev.key() == "Enter" && (ev.ctrl_key() || ev.meta_key()) {
                        ev.prevent_default();
                        on_run();
                    } else if ev.key() == "Tab" && !ev.shift_key() {
                        ev.prevent_default();
                        insert("  ".into());
                    } else if ev.key() == "Escape" {
                        show_snippets.set(false);
                    }
                }
            ></textarea>
        </div>
    }
}

/// Waits `ms` milliseconds.
async fn gloo_sleep(ms: i32) {
    let promise = js_sys::Promise::new(&mut |resolve, _| {
        let _ = window().set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, ms);
    });
    let _ = wasm_bindgen_futures::JsFuture::from(promise).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(sql: &str) -> Vec<(Tok, String)> {
        tokenize(sql, &["ds_web".to_string()]).into_iter().filter(|(t, _)| *t != Tok::Space).collect()
    }

    #[test]
    fn tokenizes_sql_and_keeps_every_character() {
        let sql = "SELECT count(*), meta->>'lang' -- note\nFROM ds_web WHERE x = 'it''s' AND n >= 1.5::DOUBLE /* c */";
        let toks = tokenize(sql, &["ds_web".into()]);
        assert_eq!(toks.iter().map(|(_, s)| s.as_str()).collect::<String>(), sql);
        let k = kinds(sql);
        assert_eq!(k[0], (Tok::Keyword, "SELECT".into()));
        assert_eq!(k[1], (Tok::Function, "count".into()));
        assert!(k.contains(&(Tok::Operator, "->>".into())));
        assert!(k.contains(&(Tok::Str, "'lang'".into())));
        assert!(k.contains(&(Tok::Comment, "-- note".into())));
        assert!(k.contains(&(Tok::Table, "ds_web".into())));
        assert!(k.contains(&(Tok::Str, "'it''s'".into())));
        assert!(k.contains(&(Tok::Number, "1.5".into())));
        assert!(k.contains(&(Tok::Operator, "::".into())));
        assert!(k.contains(&(Tok::Type, "DOUBLE".into())));
        assert!(k.contains(&(Tok::Comment, "/* c */".into())));
    }

    #[test]
    fn unfinished_input_still_tokenizes() {
        for sql in ["SELECT 'open", "/* open", "\"quoted", "a ->"] {
            assert_eq!(tokenize(sql, &[]).iter().map(|(_, s)| s.as_str()).collect::<String>(), sql);
        }
    }

    #[test]
    fn finds_offsets_and_fills_snippets() {
        assert_eq!(offset_of("ab\ncdef", 2, 1), 4);
        assert_eq!(offset_of("ab", 1, 9), 2);
        let s = &SNIPPETS[0];
        assert!(fill(s, Some("ds_x"), None).unwrap().contains("FROM ds_x"));
        let runs = SNIPPETS.iter().find(|s| s.group == "Runs").unwrap();
        assert!(fill(runs, Some("ds_x"), None).is_none());
    }
}
