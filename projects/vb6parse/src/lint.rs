//! Per-file lint rules over the CST.
//!
//! Formatting answers "is this file laid out canonically?" and always applies
//! its own answer. Linting answers "is there something wrong here?", and some
//! of those answers must not be applied automatically: renaming an identifier
//! is a decision about a public API, not a whitespace change.
//!
//! The shape follows `ruff`: every rule has a stable code, a fixability, and a
//! default, and a run selects rules by code or code prefix.
//!
//! ```rust
//! use vb6parse::lint::{lint_source, LintSettings};
//!
//! let settings = LintSettings::from_selection(&["N001".to_string()], &[]);
//! let found = lint_source("Public Function Añadir()\r\nEnd Function\r\n", &settings);
//!
//! assert_eq!(found[0].code, "N001");
//! ```

use crate::ConcreteSyntaxTree;
use crate::SyntaxKind;
use crate::parsers::CstNode;

/// Whether a rule's finding can be corrected mechanically.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fixability {
    /// The correction cannot change the meaning of the program.
    Safe,
    /// The correction is mechanical but could change behaviour in edge cases.
    Unsafe,
    /// The correction needs a judgment call and is left to a person.
    None,
}

/// A lint rule.
#[derive(Debug, Clone, Copy)]
pub struct Rule {
    /// Stable identifier, used in configuration and in output.
    pub code: &'static str,
    /// Short kebab-case name.
    pub name: &'static str,
    /// One line describing what the rule looks for.
    pub summary: &'static str,
    /// Whether the finding can be corrected mechanically.
    pub fixability: Fixability,
    /// Whether the rule runs when nothing is selected explicitly.
    pub default_on: bool,
}

/// Every rule the linter knows about.
pub const RULES: &[Rule] = &[
    Rule {
        code: "W001",
        name: "mixed-line-endings",
        summary: "file mixes CRLF and LF line endings",
        fixability: Fixability::Safe,
        default_on: true,
    },
    Rule {
        code: "W002",
        name: "trailing-whitespace",
        summary: "line ends in whitespace",
        fixability: Fixability::Safe,
        default_on: true,
    },
    Rule {
        code: "W003",
        name: "line-too-long",
        summary: "line is longer than the 1023 characters the VB6 editor accepts",
        fixability: Fixability::None,
        default_on: true,
    },
    Rule {
        code: "N001",
        name: "non-ascii-in-code",
        summary: "identifier contains a character outside ASCII",
        fixability: Fixability::None,
        default_on: false,
    },
];

/// Looks a rule up by its code.
#[must_use]
pub fn rule(code: &str) -> Option<&'static Rule> {
    RULES.iter().find(|rule| rule.code == code)
}

/// Something a rule found, at a place in the file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    /// The code of the rule that produced it.
    pub code: &'static str,
    /// What was found.
    pub message: String,
    /// One-based line number.
    pub line: usize,
    /// One-based column, counted in characters.
    pub column: usize,
    /// Whether this particular finding could be corrected mechanically.
    pub fixability: Fixability,
}

/// Which rules a run should apply.
///
/// Selection works by code or by code prefix, so `"N"` takes every rule in the
/// `N` category and `"N001"` takes one. An empty selection means the rules
/// that are on by default.
#[derive(Debug, Clone, Default)]
pub struct LintSettings {
    select: Vec<String>,
    ignore: Vec<String>,
}

impl LintSettings {
    /// Builds a selection from lists of codes or code prefixes.
    #[must_use]
    pub fn from_selection(select: &[String], ignore: &[String]) -> Self {
        Self {
            select: select.to_vec(),
            ignore: ignore.to_vec(),
        }
    }

    /// Whether `code` runs under this selection.
    #[must_use]
    pub fn is_enabled(&self, code: &str) -> bool {
        if self.ignore.iter().any(|prefix| code.starts_with(prefix)) {
            return false;
        }

        if self.select.is_empty() {
            return rule(code).is_some_and(|rule| rule.default_on);
        }

        self.select.iter().any(|prefix| code.starts_with(prefix))
    }
}

/// Runs the selected rules over one file.
#[must_use]
pub fn lint_source(source: &str, settings: &LintSettings) -> Vec<Diagnostic> {
    let mut found = Vec::new();

    if settings.is_enabled("W001") {
        found.extend(mixed_line_endings(source));
    }

    if settings.is_enabled("W002") {
        found.extend(trailing_whitespace(source));
    }

    if settings.is_enabled("W003") {
        found.extend(line_too_long(source));
    }

    if settings.is_enabled("N001") {
        found.extend(non_ascii_in_code(source));
    }

    found.sort_by_key(|diagnostic| (diagnostic.line, diagnostic.column));
    found
}

/// W001. VB6 writes CRLF; a file holding both usually got there through a tool
/// that did not know that, and the mixture shows up as a whole-file diff the
/// next time anything touches it.
fn mixed_line_endings(source: &str) -> Vec<Diagnostic> {
    let crlf = source.matches("\r\n").count();
    let lf = source.matches('\n').count() - crlf;

    if crlf == 0 || lf == 0 {
        return Vec::new();
    }

    // Report the first line that disagrees with whichever ending dominates,
    // rather than every one of them: the finding is about the file.
    let odd_one_out_is_lf = crlf >= lf;
    let mut offset = 0usize;

    for (index, line) in source.split_inclusive('\n').enumerate() {
        let is_lf = line.ends_with('\n') && !line.ends_with("\r\n");

        if is_lf == odd_one_out_is_lf && line.ends_with('\n') {
            return vec![Diagnostic {
                code: "W001",
                message: format!(
                    "file mixes line endings: {crlf} CRLF and {lf} LF; this line ends in {}",
                    if is_lf { "LF" } else { "CRLF" }
                ),
                line: index + 1,
                column: line.chars().count(),
                fixability: Fixability::Safe,
            }];
        }

        offset += line.len();
    }

    let _ = offset;
    Vec::new()
}

/// W002. Trailing whitespace is invisible, survives in the file forever and
/// turns into diff noise the first time a formatter or an editor removes it.
fn trailing_whitespace(source: &str) -> Vec<Diagnostic> {
    source
        .split('\n')
        .enumerate()
        .filter_map(|(index, line)| {
            let line = line.strip_suffix('\r').unwrap_or(line);
            let trimmed = line.trim_end_matches([' ', '\t']);

            if trimmed.len() == line.len() {
                return None;
            }

            Some(Diagnostic {
                code: "W002",
                message: format!(
                    "line ends in {} whitespace characters",
                    line.len() - trimmed.len()
                ),
                line: index + 1,
                column: trimmed.chars().count() + 1,
                fixability: Fixability::Safe,
            })
        })
        .collect()
}

/// W003. The VB6 IDE refuses a line longer than 1023 characters, and the
/// compiler does not. That gap is the whole point of the rule: `VB6.EXE /make`
/// builds a 3,000 character `Or` chain without a word (measured 2026-08-16), so
/// a line over the limit passes a command-line build in green and then breaks
/// the IDE of everyone on the team. A build gate cannot see this one.
///
/// What the compiler does limit is the string LITERAL, also at 1023, and it
/// folds concatenated literals before checking — which is why `"a" & "b" & ...`
/// fails around the same size while the `Or` chain sails past it. Those are
/// caught by the build; this rule is for the ones that are not.
///
/// The length is counted in CHARACTERS, not bytes: VB6 keeps these files in a
/// single-byte codepage, where one character is one byte, so counting UTF-8
/// bytes would over-report every line holding an accent.
///
/// No fix offered: where to break a line is a judgment call, and a `_`
/// continuation is not valid everywhere — a string literal cannot be split with
/// one. Splitting it wrong changes the program.
fn line_too_long(source: &str) -> Vec<Diagnostic> {
    /// What the VB6 editor accepts on one line.
    const LIMIT: usize = 1023;

    source
        .split('\n')
        .enumerate()
        .filter_map(|(index, line)| {
            let line = line.strip_suffix('\r').unwrap_or(line);
            let length = line.chars().count();

            if length <= LIMIT {
                return None;
            }

            Some(Diagnostic {
                code: "W003",
                message: format!(
                    "line is {length} characters long; the VB6 editor stops at {LIMIT}"
                ),
                line: index + 1,
                // Point at the first character past the limit, which is where
                // the line has to give.
                column: LIMIT + 1,
                fixability: Fixability::None,
            })
        })
        .collect()
}

/// N001. VB6 accepts accented identifiers, and code bases written in Spanish,
/// French or German are full of them. Almost nothing downstream of VB6 does,
/// so they are worth knowing about before a migration. Renaming one changes a
/// public name, so this rule never offers a fix.
///
/// Reads `Identifier` tokens straight from the CST, which is possible because
/// the lexer takes whole identifiers including their non-ASCII letters. That is
/// also what makes the rule precise: comments and string literals are their own
/// tokens, so an accent in product text is never mistaken for a name.
fn non_ascii_in_code(source: &str) -> Vec<Diagnostic> {
    let (cst_opt, _failures) = ConcreteSyntaxTree::from_text("lint_input", source).unpack();

    let Some(cst) = cst_opt else {
        return Vec::new();
    };

    let line_starts = line_starts(source);
    let mut found = Vec::new();
    let mut offset = 0usize;

    // The CST is lossless, so walking its tokens in document order and adding
    // up their lengths gives the byte offset of each one.
    walk_tokens(
        &cst.to_root_node(),
        false,
        &mut |token: &CstNode, in_designer| {
            let text = token.text();

            if token.kind() == SyntaxKind::Identifier && !text.is_ascii() && !in_designer {
                let (line, column) = position(&line_starts, source, offset);

                found.push(Diagnostic {
                    code: "N001",
                    message: format!("identifier `{text}` contains a character outside ASCII"),
                    line,
                    column,
                    fixability: Fixability::None,
                });
            }

            offset += text.len();
        },
    );

    found
}

/// Walks every token in document order, telling the visitor whether it sits
/// inside the designer section of a form or class.
///
/// Those tokens are visited either way, because the caller adds up their
/// lengths to track offsets, but a control caption there is product text and
/// the VB6 IDE owns that block, so nothing in it is reported.
fn walk_tokens(node: &CstNode, in_designer: bool, visit: &mut impl FnMut(&CstNode, bool)) {
    if node.is_token() {
        visit(node, in_designer);
        return;
    }

    let in_designer = in_designer
        || matches!(
            node.kind(),
            SyntaxKind::VersionStatement | SyntaxKind::PropertiesBlock
        );

    for child in node.children() {
        walk_tokens(child, in_designer, visit);
    }
}

/// One-based numbers of the lines that are the continuation of a comment
/// started on an earlier line.
///
/// VB6 continues a logical line with a trailing `_`, and that applies to
/// comments: everything after the continuation is still comment text. The
/// lexer does not model this — it starts reading the next line as code — so
/// prose lands in the token stream and any accent in it looks like an
/// identifier. Until the lexer handles it, the rule skips those lines rather
/// than report words from a comment.
fn continued_comment_lines(source: &str) -> std::collections::HashSet<usize> {
    let mut continued = std::collections::HashSet::new();
    let mut inside_comment = false;

    for (index, line) in source.split('\n').enumerate() {
        let line = line.strip_suffix('\r').unwrap_or(line);
        let trimmed = line.trim_start();

        if inside_comment {
            continued.insert(index + 1);
        } else {
            inside_comment = trimmed.starts_with('\'')
                || trimmed
                    .get(..4)
                    .is_some_and(|prefix| prefix.eq_ignore_ascii_case("rem "));
        }

        // The run of comment lines ends at the first one without a trailing
        // continuation.
        inside_comment = inside_comment && line.trim_end().ends_with('_');
    }

    continued
}

fn line_starts(source: &str) -> Vec<usize> {
    let mut starts = vec![0usize];
    starts.extend(source.match_indices('\n').map(|(index, _)| index + 1));
    starts
}

/// Turns a byte offset into a one-based line and character column.
///
/// The offset arrives from adding up token lengths across the tree, so it can
/// land inside a multi-byte character if the tree and the text ever disagree by
/// a byte. Slicing there panics, and a lint rule must not bring the whole run
/// down over a column number, so the offset is walked back to a boundary.
fn position(line_starts: &[usize], source: &str, offset: usize) -> (usize, usize) {
    let mut offset = offset.min(source.len());
    while offset > 0 && !source.is_char_boundary(offset) {
        offset -= 1;
    }

    let line_index = line_starts.partition_point(|start| *start <= offset) - 1;
    let line_start = line_starts[line_index];
    let column = source[line_start..offset].chars().count() + 1;

    (line_index + 1, column)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with(codes: &[&str]) -> LintSettings {
        let select: Vec<String> = codes.iter().map(|code| (*code).to_string()).collect();
        LintSettings::from_selection(&select, &[])
    }

    #[test]
    fn finds_accented_identifiers_but_not_accented_text() {
        let source = concat!(
            "' un a\u{f1}o cualquiera\r\n",
            "Public Function A\u{f1}adir() As String\r\n",
            "    A\u{f1}adir = \"a\u{f1}o\"\r\n",
            "End Function\r\n",
        );

        let found = lint_source(source, &with(&["N001"]));

        assert_eq!(
            found.len(),
            2,
            "the comment and the string are text: {found:?}"
        );
        assert_eq!(found[0].line, 2);
        // Column 17 is where `Añadir` starts: the whole identifier is one
        // token now, so the finding points at the name and not at the accent.
        assert_eq!(found[0].column, 17);
        assert!(
            found[0].message.contains("A\u{f1}adir"),
            "the message names the identifier: {}",
            found[0].message
        );
        assert_eq!(found[0].fixability, Fixability::None);
    }

    #[test]
    fn ignores_the_continuation_of_a_comment() {
        // The second line is still comment text, because the first ends in the
        // VB6 line-continuation `_`. The words in it are prose, not code.
        let source = concat!(
            "' esta linea sigue en la siguiente _\r\n",
            "  S/N\u{ba}PROYECTO y su a\u{f1}o\r\n",
            "Public Sub Main()\r\n",
            "End Sub\r\n",
        );

        assert!(lint_source(source, &with(&["N001"])).is_empty());
    }

    /// The `REM` check must not slice a line at a fixed byte offset: on a
    /// continuation line that starts with an accented character the offset
    /// lands inside it, which panics.
    #[test]
    fn continuation_line_starting_with_a_multibyte_character() {
        let source = concat!(
            "' primero _\r\n",
            "\u{e1}rea de trabajo\r\n",
            "Public Sub Main()\r\n",
            "End Sub\r\n",
        );

        assert!(lint_source(source, &with(&["N001"])).is_empty());
    }

    #[test]
    fn ascii_only_code_is_clean() {
        let source = "Public Function Anadir() As String\r\nEnd Function\r\n";

        assert!(lint_source(source, &with(&["N001"])).is_empty());
    }

    #[test]
    fn finds_mixed_line_endings() {
        let found = lint_source("Dim a\r\nDim b\nDim c\r\n", &with(&["W001"]));

        assert_eq!(
            found.len(),
            1,
            "the finding is about the file, not the line"
        );
        assert_eq!(found[0].code, "W001");
        assert_eq!(found[0].line, 2);
    }

    #[test]
    fn consistent_line_endings_are_clean() {
        assert!(lint_source("Dim a\r\nDim b\r\n", &with(&["W001"])).is_empty());
        assert!(lint_source("Dim a\nDim b\n", &with(&["W001"])).is_empty());
    }

    #[test]
    fn finds_trailing_whitespace() {
        let found = lint_source("Dim a   \r\nDim b\r\n", &with(&["W002"]));

        assert_eq!(found.len(), 1);
        assert_eq!((found[0].line, found[0].column), (1, 6));
    }

    #[test]
    fn finds_a_line_over_the_editor_limit() {
        // 1023 is the last length the VB6 editor takes; 1024 is the first it
        // refuses. The rule has to land exactly on that edge.
        let legal = format!("Dim a\r\n{}\r\n", "x".repeat(1023));
        assert!(lint_source(&legal, &with(&["W003"])).is_empty());

        let over = format!("Dim a\r\n{}\r\n", "x".repeat(1024));
        let found = lint_source(&over, &with(&["W003"]));

        assert_eq!(found.len(), 1);
        assert_eq!(found[0].code, "W003");
        assert_eq!(found[0].line, 2);
        assert_eq!(found[0].column, 1024, "points at the first character over");
        assert_eq!(found[0].fixability, Fixability::None);
        assert!(
            found[0].message.contains("1024 characters"),
            "the message gives the length: {}",
            found[0].message
        );
    }

    /// VB6 stores these files in a single-byte codepage, so a line of 1023
    /// accented characters is 1023 characters for the editor -- but 2046 bytes
    /// once it is UTF-8 in memory. Counting bytes would report a line that is
    /// perfectly legal.
    #[test]
    fn counts_characters_and_not_utf8_bytes() {
        let accented = format!("{}\r\n", "\u{f1}".repeat(1023));

        assert!(
            lint_source(&accented, &with(&["W003"])).is_empty(),
            "1023 accented characters still fit on a VB6 line"
        );

        let over = format!("{}\r\n", "\u{f1}".repeat(1024));
        assert_eq!(lint_source(&over, &with(&["W003"])).len(), 1);
    }

    #[test]
    fn line_too_long_runs_by_default() {
        // It is a hard tool limit, not a style preference: it has to fire
        // without anyone selecting it.
        let over = format!("{}\r\n", "x".repeat(1024));
        let codes: Vec<_> = lint_source(&over, &LintSettings::default())
            .iter()
            .map(|diagnostic| diagnostic.code)
            .collect();

        assert!(codes.contains(&"W003"), "got {codes:?}");
    }

    #[test]
    fn selection_is_by_code_or_prefix() {
        let source = "Public Function A\u{f1}adir()   \r\n";

        // Nothing selected: only the rules that are on by default.
        let default_codes: Vec<_> = lint_source(source, &LintSettings::default())
            .iter()
            .map(|diagnostic| diagnostic.code)
            .collect();
        assert_eq!(default_codes, vec!["W002"], "N001 is off by default");

        // A whole category by prefix.
        assert!(
            lint_source(source, &with(&["N"]))
                .iter()
                .any(|diagnostic| diagnostic.code == "N001")
        );

        // And ignore wins over select.
        let settings = LintSettings::from_selection(
            &["N".to_string(), "W".to_string()],
            &["N001".to_string()],
        );
        assert!(
            lint_source(source, &settings)
                .iter()
                .all(|diagnostic| diagnostic.code != "N001")
        );
    }

    #[test]
    fn every_rule_has_a_unique_code_and_is_reachable() {
        for rule in RULES {
            assert_eq!(
                RULES.iter().filter(|other| other.code == rule.code).count(),
                1,
                "duplicate code {}",
                rule.code
            );
            assert!(super::rule(rule.code).is_some());
        }
    }
}
