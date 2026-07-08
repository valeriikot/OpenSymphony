// PROTOTYPE: reusable fix-recipe / detector library.
//
// Background: OpenSymphony already ships a durable learning loop — completed
// issues are captured into capsules, `memory context` injects that knowledge
// into each new run, and docs sync keeps topic docs current. What the loop does
// not yet produce is an *actionable* artifact: a recurring fix expressed as a
// reusable template plus a detector that says "this issue looks like a case that
// needs it". Pipecrew (https://pipecrew.ai) calls this a `/patch` recipe —
// "both a template and a detector — so a class of change gets cheaper every
// time". See docs/specs/reusable-fix-recipes.md for the full design.
//
// This module is the smallest self-contained slice of that idea:
//
//   * a `Recipe` is a Markdown file with YAML frontmatter under
//     `<memory_root>/recipes/`, mirroring how capsules and topic docs are stored;
//   * a recipe carries a detector (`path_globs` over changed files, plus
//     `keywords` over issue text) and a Markdown body (the reusable template);
//   * `matched_recipes_section` loads the library, runs the detector against a
//     run's changed paths / issue text, and renders a Markdown block that a
//     future integration can append to the `memory context` kickoff bundle
//     (see `render_memory_context` in query.rs) so the next run starts with the
//     relevant recipes already in hand.
//
// It is deliberately read/render only; extraction of recipes from merged PRs is
// left to the capture pipeline and specified in the design doc.

/// Directory, relative to the configured memory root, that holds recipe files.
pub const RECIPES_DIR: &str = "recipes";

/// Where a learned recipe belongs, mirroring Pipecrew's tier-classified updates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RecipeTier {
    /// Local to a single repository.
    Repo,
    /// Shared across every repository in the workspace.
    Workspace,
    /// A cross-workspace pattern worth promoting into the plugin/tooling itself.
    Plugin,
}

impl RecipeTier {
    fn as_str(self) -> &'static str {
        match self {
            RecipeTier::Repo => "repo",
            RecipeTier::Workspace => "workspace",
            RecipeTier::Plugin => "plugin",
        }
    }
}

/// Serializable frontmatter for a recipe document.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct RecipeFrontMatter {
    id: String,
    title: String,
    tier: RecipeTier,
    #[serde(default)]
    path_globs: Vec<String>,
    #[serde(default)]
    keywords: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    source_issue: Option<String>,
}

/// A reusable fix recipe: a detector plus a Markdown template body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recipe {
    pub id: String,
    pub title: String,
    pub tier: RecipeTier,
    /// Changed-path globs that trigger this recipe (e.g. `crates/*/src/session.rs`).
    pub path_globs: Vec<String>,
    /// Case-insensitive keywords matched against issue title + description.
    pub keywords: Vec<String>,
    /// Issue the recipe was learned from, for provenance.
    pub source_issue: Option<String>,
    /// The reusable guidance the agent should read when the recipe matches.
    pub body: String,
}

/// A recipe that fired for a run, with the reasons it matched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecipeMatch<'a> {
    pub recipe: &'a Recipe,
    pub reasons: Vec<String>,
}

impl Recipe {
    /// Returns the match reasons if this recipe's detector fires for the given
    /// changed paths and issue text, or `None` when nothing matches.
    fn detect(&self, changed_paths: &[String], text: &str) -> Option<Vec<String>> {
        let mut reasons = Vec::new();

        for glob in &self.path_globs {
            let hits = changed_paths
                .iter()
                .filter(|path| glob_matches(glob, path))
                .collect::<Vec<_>>();
            if let Some(first) = hits.first() {
                let extra = hits.len().saturating_sub(1);
                if extra > 0 {
                    reasons.push(format!("path `{glob}` matched {first} (+{extra} more)"));
                } else {
                    reasons.push(format!("path `{glob}` matched {first}"));
                }
            }
        }

        let lower = text.to_ascii_lowercase();
        for keyword in &self.keywords {
            let needle = keyword.trim().to_ascii_lowercase();
            if !needle.is_empty() && lower.contains(&needle) {
                reasons.push(format!("keyword `{keyword}` in issue text"));
            }
        }

        if reasons.is_empty() {
            None
        } else {
            Some(reasons)
        }
    }

    fn render(&self) -> String {
        let front = RecipeFrontMatter {
            id: self.id.clone(),
            title: self.title.clone(),
            tier: self.tier,
            path_globs: self.path_globs.clone(),
            keywords: self.keywords.clone(),
            source_issue: self.source_issue.clone(),
        };
        // serde_yaml is the same YAML writer the memory config and OKF paths use.
        let frontmatter =
            serde_yaml::to_string(&front).unwrap_or_else(|_| String::from("id: invalid\n"));
        let mut output = String::from("---\n");
        output.push_str(&frontmatter);
        if !frontmatter.ends_with('\n') {
            output.push('\n');
        }
        output.push_str("---\n\n");
        output.push_str(self.body.trim_end());
        output.push('\n');
        output
    }
}

/// Absolute path to the recipe library directory.
fn recipes_dir(config: &MemoryConfig) -> PathBuf {
    config.memory_root.join(RECIPES_DIR)
}

/// Loads every recipe under `<memory_root>/recipes/`, sorted by id for
/// deterministic ordering. A missing directory yields an empty library.
pub fn load_recipes(config: &MemoryConfig) -> Result<Vec<Recipe>, MemoryError> {
    let dir = recipes_dir(config);
    if !dir.exists() {
        return Ok(Vec::new());
    }

    let entries = fs::read_dir(&dir).map_err(|source| MemoryError::ReadFile {
        path: dir.clone(),
        source,
    })?;

    let mut recipes = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|source| MemoryError::ReadFile {
            path: dir.clone(),
            source,
        })?;
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("md") {
            continue;
        }
        let contents = read_to_string(&path)?;
        recipes.push(parse_recipe(&path, &contents)?);
    }

    recipes.sort_by(|left, right| left.id.cmp(&right.id));
    Ok(recipes)
}

/// Writes a recipe into the library, returning the file path. The target is
/// containment-checked against the repository root, matching every other memory
/// write path.
pub fn write_recipe(config: &MemoryConfig, recipe: &Recipe) -> Result<PathBuf, MemoryError> {
    let slug = slugify(&recipe.id);
    if slug.is_empty() {
        return Err(MemoryError::InvalidInput(format!(
            "recipe id `{}` slugifies to an empty file name",
            recipe.id
        )));
    }
    let path = recipes_dir(config).join(format!("{slug}.md"));
    ensure_repo_contained(&config.repo_root, &path)?;
    write_file(&path, &recipe.render())?;
    Ok(path)
}

fn parse_recipe(path: &Path, contents: &str) -> Result<Recipe, MemoryError> {
    let (frontmatter, body) = split_recipe_frontmatter(path, contents)?;
    let front: RecipeFrontMatter =
        serde_yaml::from_str(&frontmatter).map_err(|source| MemoryError::ParseYaml {
            path: path.to_path_buf(),
            source,
        })?;

    if normalize_optional(&front.id).is_none() {
        return Err(MemoryError::InvalidInput(format!(
            "recipe {} is missing a non-empty id",
            path.display()
        )));
    }

    Ok(Recipe {
        id: front.id,
        title: front.title,
        tier: front.tier,
        path_globs: front.path_globs,
        keywords: front.keywords,
        source_issue: front
            .source_issue
            .and_then(|value| normalize_optional(&value)),
        body: body.trim().to_string(),
    })
}

fn split_recipe_frontmatter(path: &Path, contents: &str) -> Result<(String, String), MemoryError> {
    let normalized = contents.replace("\r\n", "\n").replace('\r', "\n");
    let rest = normalized.strip_prefix("---\n").ok_or_else(|| {
        MemoryError::InvalidInput(format!(
            "recipe {} must start with `---` YAML frontmatter",
            path.display()
        ))
    })?;
    let Some(end) = rest.find("\n---") else {
        return Err(MemoryError::InvalidInput(format!(
            "recipe {} has unterminated YAML frontmatter",
            path.display()
        )));
    };
    let frontmatter = rest[..end].to_string();
    let body = rest[end..]
        .trim_start_matches('\n')
        .strip_prefix("---")
        .unwrap_or("")
        .trim_start_matches('\n')
        .to_string();
    Ok((frontmatter, body))
}

/// Runs every recipe's detector against a run's changed paths and issue text.
pub fn match_recipes<'a>(
    recipes: &'a [Recipe],
    changed_paths: &[String],
    text: &str,
) -> Vec<RecipeMatch<'a>> {
    recipes
        .iter()
        .filter_map(|recipe| {
            recipe
                .detect(changed_paths, text)
                .map(|reasons| RecipeMatch { recipe, reasons })
        })
        .collect()
}

/// Renders matched recipes as a Markdown section suitable for appending to the
/// `memory context` kickoff bundle. Returns an empty string when nothing matched.
pub fn render_recipes_section(matches: &[RecipeMatch<'_>]) -> String {
    if matches.is_empty() {
        return String::new();
    }
    let mut output = String::from("## Applicable Fix Recipes\n\n");
    output.push_str(
        "Recurring-fix recipes whose detector fired for this issue. Treat them as reusable checklists, not authority over current code.\n\n",
    );
    for entry in matches {
        let recipe = entry.recipe;
        output.push_str(&format!(
            "### {} (`{}`, tier: {})\n\n",
            recipe.title,
            recipe.id,
            recipe.tier.as_str()
        ));
        output.push_str(&format!("- Matched: {}\n", entry.reasons.join("; ")));
        if let Some(source) = recipe.source_issue.as_deref() {
            output.push_str(&format!("- Learned from: {source}\n"));
        }
        output.push('\n');
        output.push_str(recipe.body.trim_end());
        output.push_str("\n\n");
    }
    output.trim_end().to_string()
}

/// Convenience one-shot: load the library, run detectors, render the section.
/// This is the single call a `memory context` integration or a post-run hook
/// would make.
pub fn matched_recipes_section(
    config: &MemoryConfig,
    changed_paths: &[String],
    text: &str,
) -> Result<String, MemoryError> {
    let recipes = load_recipes(config)?;
    let matches = match_recipes(&recipes, changed_paths, text);
    Ok(render_recipes_section(&matches))
}

/// Matches a `/`-delimited glob against a `/`-delimited candidate path.
/// Supports `*` (any run of characters within one segment) and `**` (any number
/// of whole segments).
fn glob_matches(pattern: &str, candidate: &str) -> bool {
    let pattern_segments = pattern.split('/').collect::<Vec<_>>();
    let candidate_segments = candidate.split('/').collect::<Vec<_>>();
    segment_match(&pattern_segments, &candidate_segments)
}

fn segment_match(pattern: &[&str], candidate: &[&str]) -> bool {
    match pattern.split_first() {
        None => candidate.is_empty(),
        Some((&"**", rest)) => {
            if segment_match(rest, candidate) {
                return true;
            }
            match candidate.split_first() {
                Some((_, candidate_rest)) => segment_match(pattern, candidate_rest),
                None => false,
            }
        }
        Some((segment, rest)) => match candidate.split_first() {
            Some((head, candidate_rest)) if wildcard_match(segment, head) => {
                segment_match(rest, candidate_rest)
            }
            _ => false,
        },
    }
}

/// Classic `*`-wildcard match within a single path segment.
fn wildcard_match(pattern: &str, text: &str) -> bool {
    let pattern = pattern.chars().collect::<Vec<_>>();
    let text = text.chars().collect::<Vec<_>>();
    let (mut pi, mut ti) = (0usize, 0usize);
    let (mut star, mut star_ti) = (None, 0usize);

    while ti < text.len() {
        if pi < pattern.len() && pattern[pi] == '*' {
            star = Some(pi);
            star_ti = ti;
            pi += 1;
        } else if pi < pattern.len() && pattern[pi] == text[ti] {
            pi += 1;
            ti += 1;
        } else if let Some(star_pi) = star {
            pi = star_pi + 1;
            star_ti += 1;
            ti = star_ti;
        } else {
            return false;
        }
    }

    while pi < pattern.len() && pattern[pi] == '*' {
        pi += 1;
    }
    pi == pattern.len()
}

#[cfg(test)]
mod recipe_tests {
    use super::*;

    fn recipe(id: &str, path_globs: &[&str], keywords: &[&str]) -> Recipe {
        Recipe {
            id: id.to_string(),
            title: format!("Recipe {id}"),
            tier: RecipeTier::Repo,
            path_globs: path_globs.iter().map(|glob| glob.to_string()).collect(),
            keywords: keywords.iter().map(|keyword| keyword.to_string()).collect(),
            source_issue: Some("COE-999".to_string()),
            body: "1. Do the thing.\n2. Add a regression test.".to_string(),
        }
    }

    fn config_for(repo_root: &Path) -> MemoryConfig {
        // Defaults put memory_root at `<repo>/.opensymphony/memory`, which is all
        // the recipe library needs.
        MemoryConfig::load(repo_root, None).expect("memory config")
    }

    #[test]
    fn glob_matches_segments_and_wildcards() {
        assert!(glob_matches("*.rs", "session.rs"));
        assert!(glob_matches(
            "crates/*/src/session.rs",
            "crates/foo/src/session.rs"
        ));
        assert!(!glob_matches(
            "crates/*/src/session.rs",
            "crates/foo/bar/src/session.rs"
        ));
        assert!(glob_matches(
            "crates/**/session.rs",
            "crates/foo/bar/src/session.rs"
        ));
        assert!(glob_matches("**/*.rs", "crates/foo/src/session.rs"));
        assert!(!glob_matches("**/*.rs", "crates/foo/src/session.py"));
        assert!(glob_matches(
            "docs/**",
            "docs/specs/reusable-fix-recipes.md"
        ));
    }

    #[test]
    fn detector_fires_on_path_or_keyword() {
        let recipe = recipe(
            "reconnect-backoff",
            &["crates/*/src/session.rs"],
            &["reconnect"],
        );

        let by_path = recipe.detect(
            &["crates/openhands/src/session.rs".to_string()],
            "unrelated title",
        );
        assert!(by_path.is_some());

        let by_keyword = recipe.detect(&["README.md".to_string()], "Fix websocket reconnect loop");
        assert!(by_keyword.is_some());

        let no_match = recipe.detect(&["README.md".to_string()], "unrelated title");
        assert!(no_match.is_none());
    }

    #[test]
    fn round_trips_through_disk_and_matches() {
        let repo = tempfile::tempdir().expect("tempdir");
        let config = config_for(repo.path());
        let written = write_recipe(
            &config,
            &recipe("reconnect-backoff", &["**/session.rs"], &["reconnect"]),
        )
        .expect("write recipe");
        assert!(written.exists());

        let loaded = load_recipes(&config).expect("load recipes");
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].id, "reconnect-backoff");
        assert_eq!(loaded[0].tier, RecipeTier::Repo);
        assert_eq!(loaded[0].source_issue.as_deref(), Some("COE-999"));

        let section = matched_recipes_section(
            &config,
            &["crates/openhands/src/session.rs".to_string()],
            "Reconnect loop drops events",
        )
        .expect("render section");
        assert!(section.contains("## Applicable Fix Recipes"));
        assert!(section.contains("reconnect-backoff"));
        assert!(section.contains("Learned from: COE-999"));
        assert!(section.contains("regression test"));
    }

    #[test]
    fn empty_library_and_no_match_render_nothing() {
        let repo = tempfile::tempdir().expect("tempdir");
        let config = config_for(repo.path());
        assert!(load_recipes(&config).expect("empty load").is_empty());

        write_recipe(
            &config,
            &recipe("reconnect-backoff", &["**/session.rs"], &["reconnect"]),
        )
        .expect("write recipe");
        let section =
            matched_recipes_section(&config, &["docs/README.md".to_string()], "docs only")
                .expect("render section");
        assert!(section.is_empty());
    }
}
