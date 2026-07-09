//! The shared vocabulary for the report-style commands (`doctor`, `parity`).
//!
//! A command builds a list of [`Check`]s — each with a stable `id`, a severity,
//! a human-readable detail, and (where we know one) the exact command that
//! fixes it — and hands it to [`Report::new`]. The CLI renders the checks as
//! marked lines; `--json` emits the report verbatim so the GUI can render the
//! same data with a working "fix" button per row.

use serde::Serialize;

/// How bad a check's result is. `Info` is a fact worth showing, not a problem.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Pass,
    Warn,
    Fail,
    Info,
}

impl Status {
    fn mark(self) -> &'static str {
        match self {
            Status::Pass => "\u{2713}",
            Status::Fail => "\u{2717}",
            Status::Warn => "\u{26a0}",
            Status::Info => "\u{b7}",
        }
    }
}

/// A command that resolves the check. `sudo` means it must run in a terminal
/// (it prompts for a password), so the GUI opens Terminal rather than
/// capturing output.
#[derive(Debug, Clone, Serialize)]
pub struct Fix {
    pub label: String,
    pub command: String,
    pub sudo: bool,
    /// The command has a `<placeholder>` the user must fill in, so it must
    /// never be executed as-is — the GUI copies it instead of running it.
    pub placeholder: bool,
}

impl Fix {
    fn new(label: &str, command: &str) -> Self {
        Fix {
            label: label.into(),
            command: command.into(),
            sudo: command.starts_with("sudo "),
            placeholder: command.contains('<'),
        }
    }
}

/// One probe and its outcome.
#[derive(Debug, Clone, Serialize)]
pub struct Check {
    /// Stable identifier (`net.port_80`) — the GUI keys off this, never the prose.
    pub id: String,
    pub category: String,
    pub title: String,
    pub status: Status,
    pub detail: String,
    /// Why it matters / what to understand, shown under the detail.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fix: Option<Fix>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Summary {
    pub pass: usize,
    pub warn: usize,
    pub fail: usize,
    pub info: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct Report {
    /// What produced this report (`dpl doctor`, `dpl parity uselookout`).
    pub title: String,
    /// One line of context under the title, e.g. the compared environments.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subtitle: Option<String>,
    /// True when nothing failed (warnings are tolerable).
    pub ok: bool,
    pub summary: Summary,
    /// Category names in display order.
    pub categories: Vec<String>,
    pub checks: Vec<Check>,
}

impl Report {
    pub fn new(title: &str, subtitle: Option<String>, categories: &[&str], checks: Vec<Check>) -> Self {
        let count = |s: Status| checks.iter().filter(|k| k.status == s).count();
        let summary = Summary {
            pass: count(Status::Pass),
            warn: count(Status::Warn),
            fail: count(Status::Fail),
            info: count(Status::Info),
        };
        Report {
            title: title.into(),
            subtitle,
            ok: summary.fail == 0,
            summary,
            // Only advertise categories that actually produced a check.
            categories: categories
                .iter()
                .filter(|c| checks.iter().any(|k| k.category == **c))
                .map(|c| c.to_string())
                .collect(),
            checks,
        }
    }

    /// Print the report as marked lines, grouped by category.
    pub fn render(&self) {
        println!("{}", self.title);
        if let Some(sub) = &self.subtitle {
            println!("{sub}");
        }
        println!();
        for category in &self.categories {
            let checks: Vec<&Check> = self.checks.iter().filter(|c| &c.category == category).collect();
            if checks.is_empty() {
                continue;
            }
            println!("{category}");
            for check in checks {
                println!("  {} {}: {}", check.status.mark(), check.title, check.detail);
                if let Some(hint) = &check.hint {
                    println!("      {hint}");
                }
                if let Some(fix) = &check.fix {
                    if !fix.command.is_empty() {
                        println!("      → {}", fix.command);
                    }
                }
            }
            println!();
        }
        let s = &self.summary;
        println!("{} passed, {} warning(s), {} problem(s)", s.pass, s.warn, s.fail);
    }
}

/// Accumulates checks as each probe runs.
#[derive(Default)]
pub struct Checks(pub Vec<Check>);

impl Checks {
    pub fn add(
        &mut self,
        id: &str,
        category: &str,
        title: &str,
        status: Status,
        detail: impl Into<String>,
    ) {
        self.0.push(Check {
            id: id.into(),
            category: category.into(),
            title: title.into(),
            status,
            detail: detail.into(),
            hint: None,
            fix: None,
        });
    }

    /// Attach a hint to the check added last.
    pub fn hint(&mut self, text: &str) -> &mut Self {
        if let Some(last) = self.0.last_mut() {
            last.hint = Some(text.into());
        }
        self
    }

    /// Attach a fix command to the check added last.
    pub fn fix(&mut self, label: &str, command: &str) -> &mut Self {
        if let Some(last) = self.0.last_mut() {
            last.fix = Some(Fix::new(label, command));
        }
        self
    }
}
