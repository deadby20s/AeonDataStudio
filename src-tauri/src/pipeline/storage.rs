//! Saved pipelines: TOML files in the workspace's `pipelines/` folder.

use std::path::{Path, PathBuf};

use aeon_shared::{PipelineSpec, PipelineSummary};

use crate::error::{AppError, AppResult};

/// `My Web Cleanup!` → `my-web-cleanup`.
fn slug(name: &str) -> String {
    let mut out = String::new();
    for c in name.trim().chars().flat_map(char::to_lowercase) {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    let out = out.trim_end_matches('-').chars().take(60).collect::<String>();
    if out.is_empty() { "pipeline".into() } else { out }
}

fn is_valid_file(file: &str) -> bool {
    file.strip_suffix(".toml").is_some_and(|stem| {
        !stem.is_empty() && stem.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    })
}

fn path_of(dir: &Path, file: &str) -> AppResult<PathBuf> {
    if !is_valid_file(file) {
        return Err(AppError::InvalidInput(format!("Invalid pipeline file name: {file:?}")));
    }
    Ok(dir.join(file))
}

pub fn list_pipelines(dir: &Path) -> AppResult<Vec<PipelineSummary>> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let file = entry.file_name().to_string_lossy().into_owned();
        if !is_valid_file(&file) {
            continue;
        }
        let modified_ms = entry
            .metadata()?
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_millis() as i64);
        // A file that doesn't parse is still listed, so the user can see and fix or delete it.
        let (name, description, step_count) = match read_pipeline(dir, &file) {
            Ok(spec) => (spec.name, spec.description, spec.steps.len() as u64),
            Err(e) => (file.clone(), Some(format!("Could not read: {e}")), 0),
        };
        out.push(PipelineSummary { file, name, description, step_count, modified_ms });
    }
    out.sort_by_key(|a| a.name.to_lowercase());
    Ok(out)
}

pub fn read_pipeline(dir: &Path, file: &str) -> AppResult<PipelineSpec> {
    Ok(toml::from_str(&std::fs::read_to_string(path_of(dir, file)?)?)?)
}

/// Saves `spec`. With `file`, overwrites that file; otherwise creates a new file named after
/// the pipeline, refusing to overwrite an existing one. Returns the file name.
pub fn save_pipeline(dir: &Path, spec: &PipelineSpec, file: Option<&str>) -> AppResult<String> {
    if spec.name.trim().is_empty() {
        return Err(AppError::InvalidInput("Give the pipeline a name".into()));
    }
    let file = match file {
        Some(f) => f.to_string(),
        None => {
            let f = format!("{}.toml", slug(&spec.name));
            if dir.join(&f).exists() {
                return Err(AppError::InvalidInput(format!(
                    "A pipeline file named {f} already exists. Choose another name."
                )));
            }
            f
        }
    };
    let path = path_of(dir, &file)?;
    let text = format!("# AeonDataStudio pipeline\n{}", toml::to_string_pretty(spec)?);
    // Write to a temp file and rename, so a crash never leaves a half-written recipe.
    let tmp = dir.join(format!(".{file}.tmp"));
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, &path)?;
    Ok(file)
}

pub fn delete_pipeline(dir: &Path, file: &str) -> AppResult<()> {
    std::fs::remove_file(path_of(dir, file)?)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs() {
        assert_eq!(slug("My Web Cleanup!"), "my-web-cleanup");
        assert_eq!(slug("  --  "), "pipeline");
        assert_eq!(slug("Größe v2"), "gr-e-v2");
    }

    #[test]
    fn save_list_read_delete_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let mut spec = PipelineSpec::template("Web cleanup");
        // Turn one rule off to check it stays off through TOML.
        if let aeon_shared::StepConfig::LengthFilter(f) = &mut spec.steps[3].config {
            f.max_words = None;
        }
        spec.steps[1].enabled = false;
        let file = save_pipeline(dir.path(), &spec, None).unwrap();
        assert_eq!(file, "web-cleanup.toml");
        assert!(save_pipeline(dir.path(), &spec, None).is_err(), "must not overwrite by name");
        assert_eq!(read_pipeline(dir.path(), &file).unwrap(), spec);

        let text = std::fs::read_to_string(dir.path().join(&file)).unwrap();
        assert!(text.contains("kind = \"length_filter\""), "{text}");

        spec.name = "Renamed".into();
        assert_eq!(save_pipeline(dir.path(), &spec, Some(&file)).unwrap(), file);
        let list = list_pipelines(dir.path()).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!((list[0].name.as_str(), list[0].step_count), ("Renamed", aeon_shared::STEP_KINDS.len() as u64));

        delete_pipeline(dir.path(), &file).unwrap();
        assert!(list_pipelines(dir.path()).unwrap().is_empty());
        assert!(read_pipeline(dir.path(), "../evil.toml").is_err());
    }
}
